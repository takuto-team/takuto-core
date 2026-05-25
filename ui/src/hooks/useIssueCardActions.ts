// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { useCallback } from "react";
import { api, apiPost } from "../api/client";

/**
 * Stable action callbacks for a single workflow card.
 *
 * Owns the per-workflow endpoint URLs and the editor / terminal /
 * close-editor flows. Higher-level concerns (loading-indicator
 * state, toast surfacing, confirmation modals) stay in the parent
 * card so they can be composed with the action's return value.
 */
export function useIssueCardActions(ticketKey: string) {
  /** Returns a thunk that POSTs `/api/workflows/{key}/{endpoint}` and
   *  throws if the response isn't OK. The thunk is intentionally not
   *  pre-bound to `withLoading` — the card decides when to wrap each
   *  action with the overlay. */
  const doAction = useCallback(
    (endpoint: string) => async () => {
      const res = await apiPost(`/api/workflows/${encodeURIComponent(ticketKey)}/${endpoint}`);
      if (!res.ok) {
        const t = await res.text();
        throw new Error(t || `Failed: ${endpoint}`);
      }
    },
    [ticketKey],
  );

  const openEditor = useCallback(async () => {
    const res = await api(`/api/workflows/${encodeURIComponent(ticketKey)}/open-editor`, {
      method: "POST",
    });
    if (!res.ok) throw new Error((await res.text()) || "Failed to start editor");
    const data = await res.json();
    if (data.url) window.open(data.url, "_blank");
  }, [ticketKey]);

  const openTerminal = useCallback(async () => {
    let res = await api(`/api/workflows/${encodeURIComponent(ticketKey)}/open-terminal`, {
      method: "POST",
    });
    // Terminal-on-cold-start: if no editor container yet, spin one up first.
    if (res.status === 409) {
      await api(`/api/workflows/${encodeURIComponent(ticketKey)}/open-editor`, {
        method: "POST",
      });
      res = await api(`/api/workflows/${encodeURIComponent(ticketKey)}/open-terminal`, {
        method: "POST",
      });
    }
    if (!res.ok) throw new Error((await res.text()) || "Failed to start terminal");
    const data = await res.json();
    if (data.url) window.open(data.url, "_blank");
  }, [ticketKey]);

  const closeEditor = useCallback(async () => {
    const res = await apiPost(`/api/workflows/${encodeURIComponent(ticketKey)}/close-editor`);
    if (!res.ok) {
      const text = await res.text();
      throw new Error(text || "Failed to close editor");
    }
  }, [ticketKey]);

  return { doAction, openEditor, openTerminal, closeEditor };
}
