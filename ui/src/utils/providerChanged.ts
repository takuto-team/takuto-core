// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import type { WorkflowEvent } from "../api/types";

/**
 * Side-effects fired when the server broadcasts `provider_changed`. Extracted
 * into a pure function so Dashboard can compose it and tests can drive it
 * without spinning up the full dashboard. See 04_architecture.md §2.3.
 *
 * - Shows an `info` toast with the from/to provider labels.
 * - Triggers a `GET /api/auth/status` refresh (server-side derived
 *   `degraded` / `provider_selected` may have flipped).
 * - Calls `refreshOnboardingStatus` so the banner reflects the new state.
 */
export interface ProviderChangedDeps {
  showToast: (message: string, type?: "error" | "success" | "info") => void;
  refreshOnboardingStatus: () => void;
  /** Plug-point for `window.fetch`; defaults to the global. Test-friendly. */
  fetchImpl?: typeof fetch;
}

export function handleProviderChangedEvent(
  evt: WorkflowEvent,
  deps: ProviderChangedDeps,
): void {
  const fromLabel = evt.from ?? "previous provider";
  const toLabel = evt.to ?? "new provider";
  deps.showToast(
    `AI provider changed: ${fromLabel} → ${toLabel}. You may need to update your credentials.`,
    "info",
  );
  const f = deps.fetchImpl ?? fetch;
  // Fire-and-forget — we don't await so the WS handler stays sync. Caller
  // doesn't need to know whether the auth-status refresh succeeded; the next
  // page load will pick up the latest values regardless.
  void f("/api/auth/status", { credentials: "same-origin" }).catch(
    () => undefined,
  );
  deps.refreshOnboardingStatus();
}
