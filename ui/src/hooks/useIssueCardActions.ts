// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { useCallback } from "react";
import { useTranslation } from "react-i18next";
import { api, apiPost } from "../api/client";
import type { MarkDoneOutcome } from "../api/types";

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

  /** POST mark-done and RETURN the outcome (rather than discarding it like
   *  `doAction`). The endpoint returns 200 with `jira_ok=false` + `jira_error`
   *  when the Jira transition didn't happen (e.g. the configured Done status
   *  doesn't match any Jira transition), so the caller inspects the outcome and
   *  surfaces the reason instead of silently treating it as success. */
  const markDone = useCallback(async (): Promise<MarkDoneOutcome> => {
    const res = await apiPost(`/api/work-items/${encodeURIComponent(ticketKey)}/mark-done`);
    if (!res.ok) {
      const text = await res.text();
      throw new Error(text || t("actions.failedEndpoint", { endpoint: "mark-done" }));
    }
    return (await res.json()) as MarkDoneOutcome;
  }, [ticketKey, t]);

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

  return { doAction, markDone, openEditor, openTerminal, closeEditor, closeTerminal };
}
