// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { apiJson } from "./http";

export type DependencyPhase = "idle" | "installing" | "ready" | "error";

/** Runtime agent/CLI install progress (`GET /api/system/dependencies`). */
export interface DependencyInstallStatus {
  phase: DependencyPhase;
  /** Label of the step in progress (e.g. "Claude Code (latest)"). */
  current_step: string;
  done: number;
  total: number;
  error?: string;
}

export async function getDependencyStatus(): Promise<DependencyInstallStatus> {
  return apiJson<DependencyInstallStatus>("/api/system/dependencies");
}
