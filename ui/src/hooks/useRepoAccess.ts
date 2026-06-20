// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * `useRepoAccess` — live "can the GitHub App still reach this repo?" status.
 *
 * Fetches `GET /api/repositories/access` once on mount and exposes a
 * name → accessible map plus a `refresh()`. Deliberately uncached so each
 * mount (settings page open, dashboard load) re-checks; a repo missing from
 * the map is treated as accessible (optimistic while loading / on error), so
 * the UI never false-flags.
 */

import { useCallback, useEffect, useState } from "react";
import { listRepoAccess } from "../api/client";

export interface UseRepoAccessResult {
  /** name → accessible. Absent name ⇒ assume accessible. */
  access: Record<string, boolean>;
  loading: boolean;
  refresh: () => void;
}

/** True unless explicitly known to be inaccessible. */
export function isRepoAccessible(access: Record<string, boolean>, name: string | null): boolean {
  return name === null || access[name] !== false;
}

export function useRepoAccess(): UseRepoAccessResult {
  const [access, setAccess] = useState<Record<string, boolean>>({});
  const [loading, setLoading] = useState(true);
  const [nonce, setNonce] = useState(0);

  const refresh = useCallback(() => setNonce((n) => n + 1), []);

  useEffect(() => {
    let cancelled = false;
    listRepoAccess()
      .then((rows) => {
        if (cancelled) return;
        setAccess(Object.fromEntries(rows.map((r) => [r.name, r.accessible])));
      })
      .catch(() => {
        /* best-effort: leave the map empty so everything reads as accessible */
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [nonce]);

  return { access, loading, refresh };
}
