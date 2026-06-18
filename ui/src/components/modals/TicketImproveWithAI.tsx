// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * `TicketImproveWithAI` — the improve-with-AI slice of `TicketDetailModal`.
 * Default export renders the inline `AiPromptPanel` plus the in-flight
 * "Improving…" overlay. Named sub-exports `.Banner` and `.FooterButtons`
 * render the diff-review banner and the left-hand footer cluster
 * respectively. State + handlers live in `useTicketImproveWithAI`.
 */

import { AiPromptPanel } from "../AiPromptPanel";
import { formatCountdown } from "../../hooks/useTicketCountdown";

export interface PendingImprovement {
  originalDescription: string;
  improvedDescription: string;
  improvedSummary?: string;
}

interface DefaultProps {
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

function TicketImproveWithAIDefault({
  ticketKey, ticketTitle, ticketDescription,
  improving, countdown,
  onCancelImprove, onLoadingChange, onImprovement,
}: DefaultProps) {
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

interface BannerProps {
  pendingImprovement: PendingImprovement | null;
  onDiscardImprovement: () => void;
  onConfirmImprovement: () => void;
}

function TicketImproveWithAIBanner({
  pendingImprovement, onDiscardImprovement, onConfirmImprovement,
}: BannerProps) {
  if (!pendingImprovement) return null;
  return (
    <div className="border-b px-4 py-2 flex items-center justify-between bg-purple-900/20 border-purple-700/30">
      <span className="text-xs text-purple-300">
        Review AI changes — confirm to enter edit mode with the updated description
      </span>
      <div className="flex gap-2">
        <button
          onClick={onDiscardImprovement}
          className="text-xs px-3 py-1 rounded-lg bg-gray-800 text-gray-300 border border-gray-700 hover:bg-gray-700 cursor-pointer"
        >
          Discard
        </button>
        <button
          onClick={onConfirmImprovement}
          className="text-xs px-3 py-1 rounded-lg bg-green-700 text-white hover:bg-green-600 cursor-pointer"
        >
          Confirm
        </button>
      </div>
    </div>
  );
}

interface FooterButtonsProps {
  pendingImprovement: PendingImprovement | null;
  improving: boolean;
  editMode: boolean;
  prompting: boolean;
  onImprove: () => void;
  onStartEdit: () => void;
  /** Leaves edit mode and returns to the read-only description view. */
  onBack: () => void;
}

function TicketImproveWithAIFooterButtons({
  pendingImprovement, improving, editMode, prompting, onImprove, onStartEdit, onBack,
}: FooterButtonsProps) {
  if (pendingImprovement) return null;
  // In edit mode the only left-cluster action is "Back" to the read-only view;
  // Improve-with-AI is read-only-only.
  if (editMode) {
    return (
      <button
        onClick={onBack}
        className="text-xs px-3 py-1.5 rounded-lg bg-gray-800 text-gray-300 border border-gray-700 hover:bg-gray-700 cursor-pointer"
      >
        Back
      </button>
    );
  }
  return (
    <>
      <button
        onClick={onImprove}
        disabled={improving || prompting}
        className="text-xs px-3 py-1.5 rounded-lg bg-purple-600/20 text-purple-300 border border-purple-500/30 hover:bg-purple-600/30 disabled:opacity-50 cursor-pointer"
      >
        {improving ? "Improving..." : "Improve with AI"}
      </button>
      <button
        onClick={onStartEdit}
        className="text-xs px-3 py-1.5 rounded-lg bg-gray-800 text-gray-300 border border-gray-700 hover:bg-gray-700 cursor-pointer"
      >
        Edit
      </button>
    </>
  );
}

export const TicketImproveWithAI = Object.assign(TicketImproveWithAIDefault, {
  Banner: TicketImproveWithAIBanner,
  FooterButtons: TicketImproveWithAIFooterButtons,
});
