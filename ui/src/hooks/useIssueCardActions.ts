// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { useCallback } from "react";
import { useTranslation } from "react-i18next";
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
  const { t } = useTranslation("dashboard");
  /** Returns a thunk that POSTs `/api/work-items/{key}/{endpoint}` and
   *  throws if the response isn't OK. The thunk is intentionally not
   *  pre-bound to `withLoading` — the card decides when to wrap each
   *  action with the overlay. */
  const doAction = useCallback(
    (endpoint: string) => async () => {
      const res = await apiPost(`/api/work-items/${encodeURIComponent(ticketKey)}/${endpoint}`);
      if (!res.ok) {
        const text = await res.text();
        throw new Error(text || t("actions.failedEndpoint", { endpoint }));
      }
    },
    [ticketKey, t],
  );

  const openEditor = useCallback(async () => {
    const res = await api(`/api/work-items/${encodeURIComponent(ticketKey)}/open-editor`, {
      method: "POST",
    });
    if (!res.ok) throw new Error((await res.text()) || t("actions.failedStartEditor"));
    const data = await res.json();
    if (data.url) window.open(data.url, "_blank");
  }, [ticketKey, t]);

  const openTerminal = useCallback(async () => {
    let res = await api(`/api/work-items/${encodeURIComponent(ticketKey)}/open-terminal`, {
      method: "POST",
    });
    // Terminal-on-cold-start: if no editor container yet, spin one up first.
    if (res.status === 409) {
      const editorRes = await api(`/api/work-items/${encodeURIComponent(ticketKey)}/open-editor`, {
        method: "POST",
      });
      // Surface a workspace-prep failure here rather than masking it behind a
      // second (also-failing) open-terminal call.
      if (!editorRes.ok) throw new Error((await editorRes.text()) || t("actions.failedPrepareWorkspace"));
      res = await api(`/api/work-items/${encodeURIComponent(ticketKey)}/open-terminal`, {
        method: "POST",
      });
    }
    if (!res.ok) throw new Error((await res.text()) || t("actions.failedStartTerminal"));
    const data = await res.json();
    if (data.url) window.open(data.url, "_blank");
  }, [ticketKey, t]);

  const closeEditor = useCallback(async () => {
    const res = await apiPost(`/api/work-items/${encodeURIComponent(ticketKey)}/close-editor`);
    if (!res.ok) {
      const text = await res.text();
      throw new Error(text || t("actions.failedCloseEditor"));
    }
  }, [ticketKey, t]);

  const closeTerminal = useCallback(async () => {
    const res = await apiPost(`/api/work-items/${encodeURIComponent(ticketKey)}/close-terminal`);
    if (!res.ok) {
      const text = await res.text();
      throw new Error(text || t("actions.failedCloseTerminal"));
    }
  }, [ticketKey, t]);

  return { doAction, openEditor, openTerminal, closeEditor, closeTerminal };
}
