// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { useState } from "react";
import { DiffView } from "../DiffView";
import { useTicketDetail } from "../../hooks/useTicketDetail";
import { useTicketCountdown } from "../../hooks/useTicketCountdown";
import { useStartWorkflow } from "../../hooks/useStartWorkflow";
import { useTicketEditor } from "../../hooks/useTicketEditor";
import { useTicketImproveWithAI } from "../../hooks/useTicketImproveWithAI";
import { TicketDetailHeader } from "./TicketDetailHeader";
import { TicketDetailView } from "./TicketDetailView";
import { TicketEditor } from "./TicketEditor";
import { TicketImproveWithAI, type PendingImprovement } from "./TicketImproveWithAI";
import { StartWorkflowRepoBanner } from "./StartWorkflowRepoBanner";
import { StartWorkflowFooter } from "./StartWorkflowFooter";

const DEFAULT_IMPROVE_TIMEOUT_SECS = 300;

interface Props {
  ticketKey: string;
  summary: string;
  description?: string;
  ticketingSystem: string;
  showStartButton: boolean;
  /** The repo whose issues the picker is browsing. For a GitHub issue this is
   * the issue's source repo — the work item is pinned to it (no repo choice). */
  activeRepoName?: string | null;
  /** Timeout in seconds for "Improve with AI" sessions, from server config. */
  improveTimeoutSecs?: number;
  /** When `showStartButton` is true, the caller receives the chosen repository_id. */
  onStart?: (description: string, summary: string, repositoryId: string) => void;
  onClose: () => void;
  /** Called after a successful save so the parent can refresh workflow data. */
  onSaved?: () => void;
}

export function TicketDetailModal({
  ticketKey, summary, description: initialDescription, ticketingSystem, showStartButton,
  activeRepoName, improveTimeoutSecs = DEFAULT_IMPROVE_TIMEOUT_SECS, onStart, onClose, onSaved,
}: Props) {
  const { markdown, setMarkdown, loading } = useTicketDetail(ticketKey, initialDescription, ticketingSystem);
  const { countdown, start: startCountdown, stop: stopCountdown } = useTicketCountdown(improveTimeoutSecs);
  // A GitHub issue is pinned to its source repo (the repo the picker browsed);
  // Jira / manual tickets aren't repo-bound, so the user picks one.
  const lockedRepoName = ticketingSystem === "github" ? activeRepoName : null;
  const e = useTicketEditor({
    summary, markdown, setMarkdown, ticketKey, onSaved,
    repository: lockedRepoName,
  });
  const [pendingImprovement, setPendingImprovement] = useState<PendingImprovement | null>(null);
  const i = useTicketImproveWithAI({
    ticketKey, markdown, editTitle: e.editTitle, improveTimeoutSecs,
    startCountdown, stopCountdown,
    pendingImprovement, setPendingImprovement,
    applyImprovementToEditor: e.applyImprovement,
  });
  const { repos, repositoryId, setRepositoryId, loadingRepos, repoLocked } = useStartWorkflow(
    showStartButton,
    lockedRepoName,
  );

  // "Add to Dashboard" must not create a work item from a stale ticket: if the
  // description was edited (manually or via Improve-with-AI, both of which land
  // in edit mode) but not yet saved, persist it to the source ticket first and
  // only proceed once the save succeeds.
  const handleStartWithSave = async (description: string, title: string, repositoryId: string) => {
    if (!onStart) return;
    if (e.editMode && e.editDirty) {
      const saved = await e.handleSaveDescription();
      if (!saved) return;
    }
    onStart(description, title, repositoryId);
  };

  // When a diff is pending, widen the modal like side-by-side edit mode.
  const isWide = e.sideBySide || pendingImprovement !== null;
  const contentClass = `flex-1 min-h-0 ${(e.editMode && e.sideBySide) || pendingImprovement ? "flex overflow-hidden" : "flex flex-col overflow-hidden"}`;
  const maxWidth = isWide ? "min(2580px, calc(100vw - 48px))" : "min(1280px, calc(100vw - 24px))";

  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div
        className="bg-gray-900 border border-gray-700 rounded-xl w-full mx-4 flex flex-col relative transition-[max-width] duration-300 ease-in-out"
        style={{ maxWidth, height: "calc(100vh - 48px)" }}
        onClick={(ev) => ev.stopPropagation()}
      >
        <TicketDetailHeader
          ticketKey={ticketKey} summary={summary}
          editing={e.editMode && !pendingImprovement}
          editTitle={e.editTitle} onEditTitleChange={e.setEditTitle}
          pendingImprovedSummary={pendingImprovement?.improvedSummary}
          onClose={onClose}
        />
        <StartWorkflowRepoBanner
          showStartButton={showStartButton} repos={repos}
          repositoryId={repositoryId} setRepositoryId={setRepositoryId}
          loadingRepos={loadingRepos} repoLocked={repoLocked} onClose={onClose}
        />
        <TicketImproveWithAI.Banner
          pendingImprovement={pendingImprovement}
          onDiscardImprovement={i.handleDiscardImprovement}
          onConfirmImprovement={i.handleConfirmImprovement}
        />
        <TicketEditor.Banner
          editMode={e.editMode} pendingImprovement={pendingImprovement}
          editDirty={e.editDirty} saving={e.saving}
          onCancelEdit={e.handleCancelEdit} onSaveDescription={e.handleSaveDescription}
        />
        <TicketEditor.TabBar
          editMode={e.editMode} pendingImprovement={pendingImprovement}
          sideBySide={e.sideBySide} activeTab={e.activeTab} setActiveTab={e.setActiveTab}
          onSideBySideChange={e.handleSideBySideChange}
        />
        <div className={contentClass}>
          {loading ? (
            <TicketDetailView markdown={markdown} loading={true} />
          ) : pendingImprovement ? (
            <DiffView oldText={pendingImprovement.originalDescription} newText={pendingImprovement.improvedDescription} />
          ) : e.editMode ? (
            <TicketEditor.Content
              sideBySide={e.sideBySide} activeTab={e.activeTab}
              editText={e.editText} setEditText={e.setEditText} debouncedText={e.debouncedText}
            />
          ) : (
            <TicketDetailView markdown={markdown} loading={false} />
          )}
        </div>
        {!loading && !pendingImprovement && (
          <TicketImproveWithAI
            ticketKey={ticketKey}
            ticketTitle={e.editMode ? e.editTitle : summary}
            ticketDescription={e.editMode ? e.editText : markdown}
            improving={i.improving} countdown={countdown}
            onCancelImprove={i.handleCancelImprove}
            onLoadingChange={i.setPrompting}
            onImprovement={i.handleImprovement}
          />
        )}
        <div className="flex items-center justify-between p-4 border-t border-gray-800 gap-3">
          <div className="flex gap-2">
            <TicketImproveWithAI.FooterButtons
              pendingImprovement={pendingImprovement}
              improving={i.improving} editMode={e.editMode} prompting={i.prompting}
              onImprove={i.handleImprove} onStartEdit={e.handleStartEdit}
            />
          </div>
          <StartWorkflowFooter
            showStartButton={showStartButton} pendingImprovement={pendingImprovement}
            editMode={e.editMode} editText={e.editText} editTitle={e.editTitle}
            markdown={markdown} summary={summary}
            repositoryId={repositoryId} loadingRepos={loadingRepos} saving={e.saving}
            onStart={onStart ? handleStartWithSave : undefined} onClose={onClose}
            onCancelEdit={e.handleCancelEdit}
            onDiscardImprovement={i.handleDiscardImprovement}
            onConfirmImprovement={i.handleConfirmImprovement}
          />
        </div>
      </div>
    </div>
  );
}
