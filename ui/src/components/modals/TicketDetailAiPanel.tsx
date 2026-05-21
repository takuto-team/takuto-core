// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * AI-prompt subsection of `TicketDetailModal`. Wraps the shared
 * `AiPromptPanel` with the modal-specific props and an "Improving…" overlay
 * that surfaces the per-request countdown. Extracted so the modal shell
 * stays under ~150 LOC per CODING_STANDARDS §3.
 *
 * Behaviour is unchanged — this file only relocates JSX that previously
 * lived inline in `TicketDetailModal.tsx`.
 */

import { AiPromptPanel } from "../AiPromptPanel";
import { formatCountdown } from "../../hooks/useTicketCountdown";

interface TicketDetailAiPanelProps {
  ticketKey: string;
  ticketTitle: string;
  ticketDescription: string;
  improving: boolean;
  countdown: number;
  onCancelImprove: () => void;
  onLoadingChange: (loading: boolean) => void;
  onImprovement: (
    originalDescription: string,
    improvedDescription: string,
    improvedSummary?: string,
  ) => void;
}

export function TicketDetailAiPanel({
  ticketKey,
  ticketTitle,
  ticketDescription,
  improving,
  countdown,
  onCancelImprove,
  onLoadingChange,
  onImprovement,
}: TicketDetailAiPanelProps) {
  return (
    <>
      {improving && (
        <div className="absolute inset-0 z-10 flex flex-col items-center justify-center bg-gray-900/85 backdrop-blur-sm rounded-xl">
          <div className="w-8 h-8 border-2 border-gray-600 border-t-blue-400 rounded-full animate-spin" />
          <p className="mt-4 text-sm text-gray-300">Improving description...</p>
          <p className="mt-1 text-xs text-gray-500">{formatCountdown(countdown)}</p>
          <button
            onClick={onCancelImprove}
            className="mt-4 text-xs px-3 py-1.5 rounded-lg bg-gray-800 text-gray-300 border border-gray-700 hover:bg-gray-700 cursor-pointer"
          >
            Cancel
          </button>
        </div>
      )}
      <AiPromptPanel
        ticketKey={ticketKey}
        ticketTitle={ticketTitle}
        ticketDescription={ticketDescription}
        disabled={improving}
        onLoadingChange={onLoadingChange}
        onImprovement={onImprovement}
      />
    </>
  );
}
