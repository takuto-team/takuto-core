// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { useEffect, useRef, useState } from "react";
import { DiffView } from "../DiffView";
import { useToast } from "../../hooks/useToast";
import { useTicketDetail } from "../../hooks/useTicketDetail";
import { useTicketCountdown } from "../../hooks/useTicketCountdown";
import { useStartWorkflow } from "../../hooks/useStartWorkflow";
import { useTicketEditor } from "../../hooks/useTicketEditor";
import { TicketDetailAiPanel } from "./TicketDetailAiPanel";
import { TicketDetailHeader } from "./TicketDetailHeader";
import { TicketDetailView } from "./TicketDetailView";
import { TicketEditor } from "./TicketEditor";
import { StartWorkflowRepoBanner } from "./StartWorkflowRepoBanner";
import { StartWorkflowFooter } from "./StartWorkflowFooter";
import type { PendingImprovement } from "./TicketImproveWithAI";

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
  const [improving, setImproving] = useState(false);
  const abortRef = useRef<AbortController | null>(null);
  const { showToast } = useToast();
  const editor = useTicketEditor({ summary, markdown, setMarkdown, ticketKey, onSaved });
  const {
    editMode, editText, editTitle, activeTab, sideBySide, debouncedText, saving, editDirty,
    setEditText, setEditTitle, setActiveTab,
    handleStartEdit, handleCancelEdit, handleSaveDescription, handleSideBySideChange,
    applyImprovement,
  } = editor;
  const [prompting, setPrompting] = useState(false);
  const [pendingImprovement, setPendingImprovement] =
    useState<PendingImprovement | null>(null);

  // Plan-10: repository selector — only shown when starting a workflow.
  const { repos, repositoryId, setRepositoryId, loadingRepos } = useStartWorkflow(showStartButton);

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
        showToast(text || "Failed to improve ticket description");
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
        showToast("Failed to improve ticket description");
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
    applyImprovement(improvedDescription, improvedSummary);
    setPendingImprovement(null);
  };

  const handleDiscardImprovement = () => {
    setPendingImprovement(null);
  };

  /** Called by AiPromptPanel when the AI returns an improved version. */
  const handleImprovement = (
    originalDescription: string,
    improvedDescription: string,
    improvedSummary?: string
  ) => {
    setPendingImprovement({ originalDescription, improvedDescription, improvedSummary });
  };

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
        {pendingImprovement && (
          <div className="border-b px-4 py-2 flex items-center justify-between bg-purple-900/20 border-purple-700/30">
            <span className="text-xs text-purple-300">
              Review AI changes — confirm to enter edit mode with the updated description
            </span>
            <div className="flex gap-2">
              <button
                onClick={handleDiscardImprovement}
                className="text-xs px-3 py-1 rounded-lg bg-gray-800 text-gray-300 border border-gray-700 hover:bg-gray-700 cursor-pointer"
              >
                Discard
              </button>
              <button
                onClick={handleConfirmImprovement}
                className="text-xs px-3 py-1 rounded-lg bg-green-700 text-white hover:bg-green-600 cursor-pointer"
              >
                Confirm
              </button>
            </div>
          </div>
        )}

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
          <TicketDetailAiPanel
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
            {!pendingImprovement && (
              <>
                <button
                  onClick={handleImprove}
                  disabled={improving || editMode || prompting}
                  className="text-xs px-3 py-1.5 rounded-lg bg-purple-600/20 text-purple-300 border border-purple-500/30 hover:bg-purple-600/30 disabled:opacity-50 cursor-pointer"
                >
                  {improving ? "Improving..." : "Improve with AI"}
                </button>
                {!editMode && (
                  <button
                    onClick={handleStartEdit}
                    className="text-xs px-3 py-1.5 rounded-lg bg-gray-800 text-gray-300 border border-gray-700 hover:bg-gray-700 cursor-pointer"
                  >
                    Edit
                  </button>
                )}
              </>
            )}
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
