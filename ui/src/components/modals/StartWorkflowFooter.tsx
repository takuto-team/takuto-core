// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Right-hand cluster of `TicketDetailModal`'s footer. Renders the
 * Cancel/Close/Discard button (label depends on mode) and the trailing
 * Confirm-AI-change / Add-to-Dashboard button when applicable. Pairs with
 * `StartWorkflowRepoBanner` and `useStartWorkflow`.
 *
 * The "Add to Dashboard" CTA is disabled until a repository is selected
 * and the repo list has finished loading (preserved verbatim).
 */

import type { PendingImprovement } from "./TicketImproveWithAI";

interface Props {
  showStartButton: boolean;
  pendingImprovement: PendingImprovement | null;
  editMode: boolean;
  editText: string;
  editTitle: string;
  markdown: string;
  summary: string;
  repositoryId: string;
  loadingRepos: boolean;
  /** True while an unsaved edit is being persisted before the work item is
   *  created — disables the CTA so a double-click can't add twice. */
  saving?: boolean;
  onStart?: (description: string, summary: string, repositoryId: string) => void;
  onClose: () => void;
  onCancelEdit: () => void;
  onDiscardImprovement: () => void;
  onConfirmImprovement: () => void;
}

export function StartWorkflowFooter({
  showStartButton,
  pendingImprovement,
  editMode,
  editText,
  editTitle,
  markdown,
  summary,
  repositoryId,
  loadingRepos,
  saving,
  onStart,
  onClose,
  onCancelEdit,
  onDiscardImprovement,
  onConfirmImprovement,
}: Props) {
  const leftAction = pendingImprovement
    ? onDiscardImprovement
    : editMode
    ? onCancelEdit
    : onClose;
  const leftLabel = pendingImprovement ? "Discard" : editMode ? "Cancel" : "Close";

  return (
    <div className="flex gap-2">
      <button
        onClick={leftAction}
        className="text-xs px-4 py-1.5 rounded-lg bg-gray-800 text-gray-300 border border-gray-700 hover:bg-gray-700 cursor-pointer"
      >
        {leftLabel}
      </button>
      {pendingImprovement ? (
        <button
          onClick={onConfirmImprovement}
          className="text-xs px-4 py-1.5 rounded-lg bg-green-700 text-white hover:bg-green-600 cursor-pointer"
        >
          Confirm
        </button>
      ) : showStartButton && onStart ? (
        <button
          onClick={() => onStart(editMode ? editText : markdown, editMode ? editTitle : summary, repositoryId)}
          disabled={!repositoryId || loadingRepos || saving}
          className="text-xs px-4 py-1.5 rounded-lg bg-blue-600 text-white hover:bg-blue-500 disabled:opacity-50 disabled:cursor-not-allowed cursor-pointer"
        >
          {saving ? "Saving…" : "Add to Dashboard"}
        </button>
      ) : null}
    </div>
  );
}
