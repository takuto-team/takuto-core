// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Warns the browser before a full page unload (reload / close / typed URL /
 * browser back) while `when` is true. In-app navigation (tab switches, the
 * Dashboard link, logout) is intercepted by the caller instead — `useBlocker`
 * is not available because the app uses a plain `BrowserRouter`, not a data
 * router.
 */
import { useEffect } from "react";

export function useUnsavedChangesGuard(when: boolean): void {
  useEffect(() => {
    if (!when) return;
    const handler = (e: BeforeUnloadEvent) => {
      e.preventDefault();
      // Legacy browsers require setting returnValue to trigger the prompt.
      e.returnValue = "";
    };
    window.addEventListener("beforeunload", handler);
    return () => window.removeEventListener("beforeunload", handler);
  }, [when]);
}
