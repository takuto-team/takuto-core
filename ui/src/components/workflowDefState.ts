// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Pure per-flow state derivation shared by the inline `WorkflowDefButtons` and
 * the overflow `StartFlowModal`, so the two surfaces render the same five
 * visual states from the same logic and cannot drift.
 */

import type { WorkflowDefinition } from "../api/types";

/** Topological sort of definitions based on depends_on. Falls back to alphabetical on a cycle. */
export function topoSort(defs: WorkflowDefinition[]): WorkflowDefinition[] {
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

/** The raw run state for a definition ("idle" when unknown). */
export function runStateOf(def: WorkflowDefinition, runStates: Record<string, string>): string {
  return runStates[def.filename] || "idle";
}

/** True when every dependency of `def` has completed at least once. */
export function depsAreMet(def: WorkflowDefinition, runStates: Record<string, string>): boolean {
  return def.depends_on.every((dep) => runStates[dep] === "completed");
}

/** Names of the dependencies of `def` that have not yet completed. */
export function unmetDeps(
  def: WorkflowDefinition,
  definitions: WorkflowDefinition[],
  runStates: Record<string, string>,
): string[] {
  const byFile = new Map<string, WorkflowDefinition>();
  for (const d of definitions) byFile.set(d.filename, d);
  return def.depends_on
    .filter((dep) => runStates[dep] !== "completed")
    .map((dep) => byFile.get(dep)?.name || dep);
}
