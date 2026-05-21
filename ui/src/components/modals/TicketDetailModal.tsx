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
  /** Timeout in seconds for "Improve with AI" sessions, from server config. */
  improveTimeoutSecs?: number;
  /** Plan-10: when `showStartButton` is true, the caller receives the chosen repository_id. */
  onStart?: (description: string, summary: string, repositoryId: string) => void;
  onClose: () => void;
  /** Called after a successful save so the parent can refresh workflow data. */
  onSaved?: () => void;
}

export function TicketDetailModal({
  ticketKey,
  summary,
  description: initialDescription,
  ticketingSystem,
  showStartButton,
  improveTimeoutSecs = DEFAULT_IMPROVE_TIMEOUT_SECS,
  onStart,
  onClose,
  onSaved,
}: Props) {
  const { markdown, setMarkdown, loading } = useTicketDetail(
    ticketKey,
    initialDescription,
    ticketingSystem,
  );
  const { countdown, start: startCountdown, stop: stopCountdown } =
    useTicketCountdown(improveTimeoutSecs);
  const editor = useTicketEditor({ summary, markdown, setMarkdown, ticketKey, onSaved });
  const {
    editMode, editText, editTitle, activeTab, sideBySide, debouncedText, saving, editDirty,
    setEditText, setEditTitle, setActiveTab,
    handleStartEdit, handleCancelEdit, handleSaveDescription, handleSideBySideChange,
    applyImprovement,
  } = editor;
  const [pendingImprovement, setPendingImprovement] =
    useState<PendingImprovement | null>(null);
  const improve = useTicketImproveWithAI({
    ticketKey, markdown, editTitle, improveTimeoutSecs,
    startCountdown, stopCountdown,
    pendingImprovement, setPendingImprovement,
    applyImprovementToEditor: applyImprovement,
  });
  const {
    improving, prompting, setPrompting,
    handleImprove, handleCancelImprove,
    handleConfirmImprovement, handleDiscardImprovement, handleImprovement,
  } = improve;

  // Plan-10: repository selector — only shown when starting a workflow.
  const { repos, repositoryId, setRepositoryId, loadingRepos } = useStartWorkflow(showStartButton);

  // When a diff is pending, widen the modal like side-by-side edit mode.
  const isWide = sideBySide || pendingImprovement !== null;

  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div
        className="bg-gray-900 border border-gray-700 rounded-xl w-full mx-4 flex flex-col relative transition-[max-width] duration-300 ease-in-out"
        style={{
          maxWidth: isWide
            ? "min(2580px, calc(100vw - 48px))"
            : "min(1280px, calc(100vw - 24px))",
          height: "calc(100vh - 48px)",
        }}
        onClick={(e) => e.stopPropagation()}
      >
        {/* Header */}
        <TicketDetailHeader
          ticketKey={ticketKey}
          summary={summary}
          editing={editMode && !pendingImprovement}
          editTitle={editTitle}
          onEditTitleChange={setEditTitle}
          pendingImprovedSummary={pendingImprovement?.improvedSummary}
          onClose={onClose}
        />

        {/* Plan-10 repo selector — required when starting a workflow. */}
        <StartWorkflowRepoBanner
          showStartButton={showStartButton}
          repos={repos}
          repositoryId={repositoryId}
          setRepositoryId={setRepositoryId}
          loadingRepos={loadingRepos}
          onClose={onClose}
        />

        {/* Diff review banner */}
        <TicketImproveWithAI.Banner
          pendingImprovement={pendingImprovement}
          onDiscardImprovement={handleDiscardImprovement}
          onConfirmImprovement={handleConfirmImprovement}
        />

        {/* Edit banner — only in edit mode and not while reviewing a diff */}
        <TicketEditor.Banner
          editMode={editMode}
          pendingImprovement={pendingImprovement}
          editDirty={editDirty}
          saving={saving}
          onCancelEdit={handleCancelEdit}
          onSaveDescription={handleSaveDescription}
        />

        {/* Tab bar — only visible in edit mode and not while reviewing a diff */}
        <TicketEditor.TabBar
          editMode={editMode}
          pendingImprovement={pendingImprovement}
          sideBySide={sideBySide}
          activeTab={activeTab}
          setActiveTab={setActiveTab}
          onSideBySideChange={handleSideBySideChange}
        />

        {/* Content area */}
        <div className={`flex-1 min-h-0 ${(editMode && sideBySide) || pendingImprovement ? "flex overflow-hidden" : "flex flex-col overflow-hidden"}`}>
          {loading ? (
            <TicketDetailView markdown={markdown} loading={true} />
          ) : pendingImprovement ? (
            <DiffView
              oldText={pendingImprovement.originalDescription}
              newText={pendingImprovement.improvedDescription}
            />
          ) : editMode ? (
            <TicketEditor.Content
              sideBySide={sideBySide}
              activeTab={activeTab}
              editText={editText}
              setEditText={setEditText}
              debouncedText={debouncedText}
            />
          ) : (
            <TicketDetailView markdown={markdown} loading={false} />
          )}
        </div>

        {/* AI Prompt Panel — hidden while reviewing a diff */}
        {!loading && !pendingImprovement && (
          <TicketImproveWithAI
            ticketKey={ticketKey}
            ticketTitle={editMode ? editTitle : summary}
            ticketDescription={editMode ? editText : markdown}
            improving={improving}
            countdown={countdown}
            onCancelImprove={handleCancelImprove}
            onLoadingChange={setPrompting}
            onImprovement={handleImprovement}
          />
        )}

        {/* Footer */}
        <div className="flex items-center justify-between p-4 border-t border-gray-800 gap-3">
          <div className="flex gap-2">
            <TicketImproveWithAI.FooterButtons
              pendingImprovement={pendingImprovement}
              improving={improving}
              editMode={editMode}
              prompting={prompting}
              onImprove={handleImprove}
              onStartEdit={handleStartEdit}
            />
          </div>
          <StartWorkflowFooter
            showStartButton={showStartButton}
            pendingImprovement={pendingImprovement}
            editMode={editMode}
            editText={editText}
            editTitle={editTitle}
            markdown={markdown}
            summary={summary}
            repositoryId={repositoryId}
            loadingRepos={loadingRepos}
            onStart={onStart}
            onClose={onClose}
            onCancelEdit={handleCancelEdit}
            onDiscardImprovement={handleDiscardImprovement}
            onConfirmImprovement={handleConfirmImprovement}
          />
        </div>
      </div>
    </div>
  );
}
