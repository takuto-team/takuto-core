// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * `useTicketImproveWithAI` — owns the improve-with-AI slice of
 * `TicketDetailModal`: the in-flight fetch + abort, the prompting flag
 * (true while the inline `AiPromptPanel` is loading), and the
 * confirm/discard handlers for the resulting diff.
 *
 * The abort cleanup `useEffect` aborts any in-flight
 * `/api/tickets/{key}/improve` request on unmount (transitively from the
 * modal closing). The non-AbortError toast path is preserved.
 */

import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { useToast } from "./useToast";
import type { PendingImprovement } from "../components/modals/TicketImproveWithAI";

interface Params {
  ticketKey: string;
  markdown: string;
  editTitle: string;
  improveTimeoutSecs: number;
  startCountdown: (timeoutSecs: number) => void;
  stopCountdown: () => void;
  pendingImprovement: PendingImprovement | null;
  setPendingImprovement: (next: PendingImprovement | null) => void;
  applyImprovementToEditor: (improvedDescription: string, improvedSummary?: string) => void;
}

export interface UseTicketImproveWithAIResult {
  improving: boolean;
  prompting: boolean;
  setPrompting: (next: boolean) => void;
  handleImprove: () => Promise<void>;
  handleCancelImprove: () => void;
  handleConfirmImprovement: () => void;
  handleDiscardImprovement: () => void;
  handleImprovement: (
    originalDescription: string,
    improvedDescription: string,
    improvedSummary?: string,
  ) => void;
}

export function useTicketImproveWithAI(params: Params): UseTicketImproveWithAIResult {
  const {
    ticketKey, markdown, editTitle, improveTimeoutSecs,
    startCountdown, stopCountdown,
    pendingImprovement, setPendingImprovement, applyImprovementToEditor,
  } = params;
  const { t } = useTranslation("modals");
  const { showToast } = useToast();
  const [improving, setImproving] = useState(false);
  const [prompting, setPrompting] = useState(false);
  const abortRef = useRef<AbortController | null>(null);

  useEffect(() => {
    return () => {
      abortRef.current?.abort();
    };
  }, []);

  const handleImprove = async () => {
    setImproving(true);
    startCountdown(improveTimeoutSecs);

    abortRef.current = new AbortController();
    try {
      const res = await fetch(`/api/tickets/${encodeURIComponent(ticketKey)}/improve`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        credentials: "same-origin",
        body: JSON.stringify({ description: markdown, summary: editTitle }),
        signal: abortRef.current.signal,
      });
      abortRef.current = null;
      if (!res.ok) {
        const text = await res.text();
        showToast(text || t("improveWithAI.failed"));
        return;
      }
      const data = await res.json() as { improved_description: string; improved_summary?: string };
      setPendingImprovement({
        originalDescription: markdown,
        improvedDescription: data.improved_description,
        improvedSummary: data.improved_summary,
      });
    } catch (e) {
      abortRef.current = null;
      if (e instanceof Error && e.name !== "AbortError") {
        showToast(t("improveWithAI.failed"));
      }
    } finally {
      setImproving(false);
      stopCountdown();
    }
  };

  const handleCancelImprove = () => {
    abortRef.current?.abort();
    abortRef.current = null;
    setImproving(false);
    stopCountdown();
  };

  const handleConfirmImprovement = () => {
    if (!pendingImprovement) return;
    const { improvedDescription, improvedSummary } = pendingImprovement;
    applyImprovementToEditor(improvedDescription, improvedSummary);
    setPendingImprovement(null);
  };

  const handleDiscardImprovement = () => {
    setPendingImprovement(null);
  };

  const handleImprovement = (
    originalDescription: string,
    improvedDescription: string,
    improvedSummary?: string,
  ) => {
    setPendingImprovement({ originalDescription, improvedDescription, improvedSummary });
  };

  return {
    improving,
    prompting,
    setPrompting,
    handleImprove,
    handleCancelImprove,
    handleConfirmImprovement,
    handleDiscardImprovement,
    handleImprovement,
  };
}
