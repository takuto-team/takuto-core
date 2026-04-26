// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { useState } from "react";
import { apiPost } from "../api/client";
import type { WorkflowDefinition } from "../api/types";
import { useToast } from "../hooks/useToast";

interface WorkflowDefButtonsProps {
  definitions: WorkflowDefinition[];
  runStates: Record<string, string>;
  ticketKey: string;
  onRefresh: () => void;
  /** When true, all buttons are disabled (main pipeline is actively running). */
  mainRunning?: boolean;
}

/** Topological sort of definitions based on depends_on. Falls back to alphabetical. */
function topoSort(defs: WorkflowDefinition[]): WorkflowDefinition[] {
  const byFile = new Map<string, WorkflowDefinition>();
  for (const d of defs) byFile.set(d.filename, d);

  const sorted: WorkflowDefinition[] = [];
  const visited = new Set<string>();
  const visiting = new Set<string>();
  let hasCycle = false;

  function visit(d: WorkflowDefinition) {
    if (visited.has(d.filename)) return;
    if (visiting.has(d.filename)) {
      hasCycle = true;
      return;
    }
    visiting.add(d.filename);
    for (const dep of d.depends_on) {
      const depDef = byFile.get(dep);
      if (depDef) visit(depDef);
    }
    visiting.delete(d.filename);
    visited.add(d.filename);
    sorted.push(d);
  }

  for (const d of defs) visit(d);

  if (hasCycle) {
    return [...defs].sort((a, b) => a.name.localeCompare(b.name));
  }
  return sorted;
}

function SpinnerIcon() {
  return (
    <svg className="w-3 h-3 animate-spin" fill="none" viewBox="0 0 24 24">
      <circle className="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" strokeWidth="4" />
      <path className="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8v4a4 4 0 00-4 4H4z" />
    </svg>
  );
}

function CheckIcon() {
  return (
    <svg className="w-3 h-3" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2.5}>
      <path strokeLinecap="round" strokeLinejoin="round" d="M5 13l4 4L19 7" />
    </svg>
  );
}

function XIcon() {
  return (
    <svg className="w-3 h-3" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2.5}>
      <path strokeLinecap="round" strokeLinejoin="round" d="M6 18L18 6M6 6l12 12" />
    </svg>
  );
}

function LockIcon() {
  return (
    <svg className="w-3 h-3" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
      <rect x="5" y="11" width="14" height="10" rx="2" />
      <path d="M8 11V7a4 4 0 118 0v4" />
    </svg>
  );
}

export function WorkflowDefButtons({ definitions, runStates, ticketKey, onRefresh, mainRunning }: WorkflowDefButtonsProps) {
  const { showToast } = useToast();
  const [loadingDef, setLoadingDef] = useState<string | null>(null);

  const validDefs = definitions.filter((d) => d.valid);
  if (validDefs.length === 0) return null;

  const sorted = topoSort(validDefs);

  function depsAreMet(def: WorkflowDefinition): boolean {
    return def.depends_on.every((dep) => runStates[dep] === "completed");
  }

  function unmetDeps(def: WorkflowDefinition): string[] {
    const defsByFile = new Map<string, WorkflowDefinition>();
    for (const d of definitions) defsByFile.set(d.filename, d);
    return def.depends_on
      .filter((dep) => runStates[dep] !== "completed")
      .map((dep) => defsByFile.get(dep)?.name || dep);
  }

  async function handleRun(def: WorkflowDefinition) {
    const state = runStates[def.filename] || "idle";
    const endpoint = state === "error" ? "retry-workflow" : "run-workflow";
    setLoadingDef(def.filename);
    try {
      const res = await apiPost(
        `/api/workflows/${encodeURIComponent(ticketKey)}/${endpoint}/${encodeURIComponent(def.filename)}`
      );
      if (!res.ok) {
        const text = await res.text();
        throw new Error(text || `Failed to ${endpoint}`);
      }
      onRefresh();
    } catch (e) {
      showToast(e instanceof Error ? e.message : "Action failed");
    } finally {
      setLoadingDef(null);
    }
  }

  return (
    <>
      <div className="border-t border-gray-800/60" />
      <div>
        <div className="text-xs text-gray-500 mb-1.5">Workflows</div>
        <div className="flex flex-wrap gap-2">
          {sorted.map((def) => {
            const state = runStates[def.filename] || "idle";
            const met = depsAreMet(def);
            const isLoading = loadingDef === def.filename;

            if (state === "running") {
              return (
                <span
                  key={def.filename}
                  className="action-btn wf-btn-primary opacity-75 cursor-default inline-flex items-center justify-between gap-2"
                >
                  {def.name} <SpinnerIcon />
                </span>
              );
            }

            if (state === "completed") {
              return (
                <span
                  key={def.filename}
                  className="action-btn wf-btn-success cursor-default inline-flex items-center gap-1"
                >
                  <CheckIcon /> {def.name}
                </span>
              );
            }

            if (state === "error") {
              if (mainRunning) {
                return (
                  <span
                    key={def.filename}
                    className="action-btn wf-btn-danger opacity-50 cursor-not-allowed inline-flex items-center gap-1"
                  >
                    <XIcon /> {def.name}
                  </span>
                );
              }
              return (
                <button
                  key={def.filename}
                  className="action-btn wf-btn-danger inline-flex items-center gap-1"
                  onClick={() => handleRun(def)}
                  disabled={isLoading}
                  title="Click to retry"
                >
                  {isLoading ? <SpinnerIcon /> : <XIcon />} {def.name}
                </button>
              );
            }

            // idle state
            if (!met || mainRunning) {
              const waiting = !met ? unmetDeps(def) : [];
              return (
                <span
                  key={def.filename}
                  className="action-btn wf-btn-secondary opacity-50 cursor-not-allowed inline-flex items-center gap-1"
                  title={!met ? `Waiting for: ${waiting.join(", ")}` : undefined}
                >
                  {!met && <LockIcon />} {def.name}
                </span>
              );
            }

            return (
              <button
                key={def.filename}
                className="action-btn wf-btn-primary inline-flex items-center gap-1"
                onClick={() => handleRun(def)}
                disabled={isLoading}
              >
                {isLoading ? <SpinnerIcon /> : null} {def.name}
              </button>
            );
          })}
        </div>
      </div>
    </>
  );
}
