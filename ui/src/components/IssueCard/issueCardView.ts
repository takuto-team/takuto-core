// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Pure view-model derivation for `IssueCard`. Takes the workflow + its live
 * terminal state + dynamic port forwards and computes every display value the
 * card and its sub-components render. Keeping this out of the component body
 * means the `.tsx` is a pure renderer (CODING_STANDARDS §3 — no inline logic
 * in the component).
 */

import type { WorkflowSummary } from "../../api/types";
import type { TerminalState } from "../../hooks/useWorkflows";
import { getStatusInfo, type StatusInfo } from "../StatusBadge";

export interface IssueCardView {
  status: StatusInfo;
  pct: number;
  total: number;
  filled: number;
  prUrl: string;
  isTerminal: boolean;
  isPending: boolean;
  isPreparingWorktree: boolean;
  isActive: boolean;
  duration: string | null;
  stepLabel: string;
  stateDisplay: string;
  borderClass: string;
  effectiveTs: TerminalState | undefined;
  hasTerminalLines: boolean;
  mergedPorts: [number, string][];
}

function formatDuration(start: Date, end: Date): string {
  const secs = Math.max(0, Math.floor((end.getTime() - start.getTime()) / 1000));
  const h = Math.floor(secs / 3600);
  const m = Math.floor((secs % 3600) / 60);
  const s = secs % 60;
  if (h > 0) return `${h}h ${m}m ${s}s`;
  if (m > 0) return `${m}m ${s}s`;
  return `${s}s`;
}

export function buildIssueCardView(
  w: WorkflowSummary,
  ts: TerminalState | undefined,
  dynamicForwards: [number, string][],
): IssueCardView {
  const status = getStatusInfo(w.state, w.can_start);
  const pct = Math.max(0, Math.min(100, Math.round(w.progress_percent || 0)));
  const total = w.progress_steps_total > 0 ? Math.floor(w.progress_steps_total) : 0;
  const filled = total > 0 ? Math.min(total, Math.round((pct * total) / 100)) : 0;
  const prUrl = w.pr_url?.trim() || "";
  const isTerminal = ["Completed", "Error", "Stopped"].includes(status.label);
  const isPending = status.label === "Pending" && w.can_start;
  const isPreparingWorktree = isPending && !!w.branch_name && !w.worktree_path;
  const isActive = status.label === "Running" || status.label === "Paused";

  const duration =
    isTerminal &&
    status.label !== "Error" &&
    status.label !== "Completed" &&
    status.label !== "Stopped" &&
    w.started_at &&
    w.updated_at
      ? formatDuration(new Date(w.started_at), new Date(w.updated_at))
      : null;

  let stepLabel = "Current step";
  if (status.label === "Completed") stepLabel = "Completed";
  else if (status.label === "Error") stepLabel = "Failed at step";
  else if (status.label === "Paused") stepLabel = "Paused at step";
  else if (status.label === "Stopped") stepLabel = "Stopped at step";
  else if (isPending) stepLabel = "Added to dashboard";

  let stateDisplay = w.state;
  if (status.label === "Completed") stateDisplay = "All steps passed";
  if (status.label === "Error" && w.state.startsWith("Error:")) {
    stateDisplay = w.state.replace("Error: ", "");
  }
  if (total > 0) stateDisplay += ` (${filled}/${total})`;

  const borderClass =
    status.color === "red"
      ? "border-red-500/30 hover:border-red-500/40"
      : status.color === "yellow"
        ? "border-yellow-500/30 hover:border-yellow-500/40"
        : "border-gray-800 hover:border-gray-700";

  const effectiveTs =
    ts ??
    (isTerminal && w.terminal_lines?.length > 0
      ? { stepName: w.state, lines: w.terminal_lines, completed: true }
      : undefined);
  const hasTerminalLines = !!effectiveTs && effectiveTs.lines.length > 0;

  const dynPorts = new Set(dynamicForwards.map(([cp]) => cp));
  const mergedPorts: [number, string][] = [
    ...(w.editor_port_mappings || []).filter(([cp]) => !dynPorts.has(cp)),
    ...dynamicForwards,
  ];

  return {
    status,
    pct,
    total,
    filled,
    prUrl,
    isTerminal,
    isPending,
    isPreparingWorktree,
    isActive,
    duration,
    stepLabel,
    stateDisplay,
    borderClass,
    effectiveTs,
    hasTerminalLines,
    mergedPorts,
  };
}
