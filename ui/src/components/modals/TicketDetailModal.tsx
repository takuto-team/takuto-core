// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { useEffect, useRef, useState } from "react";
import { Link } from "react-router-dom";
import { apiPost, listMyRepositories, type RepositoryRow } from "../../api/client";
import { MarkdownPreview } from "../MarkdownPreview";
import { DiffView } from "../DiffView";
import { useToast } from "../../hooks/useToast";
import { useTicketDetail } from "../../hooks/useTicketDetail";
import { useTicketCountdown } from "../../hooks/useTicketCountdown";
import { TicketDetailAiPanel } from "./TicketDetailAiPanel";
import { TicketDetailHeader } from "./TicketDetailHeader";
import { TicketDetailView } from "./TicketDetailView";

const DEFAULT_IMPROVE_TIMEOUT_SECS = 300;

interface PendingImprovement {
  originalDescription: string;
  improvedDescription: string;
  improvedSummary?: string;
}

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
  const [editTitle, setEditTitle] = useState(summary);
  const [improving, setImproving] = useState(false);
  const abortRef = useRef<AbortController | null>(null);
  const { showToast } = useToast();
  const [editMode, setEditMode] = useState(false);
  const [editText, setEditText] = useState("");
  const [activeTab, setActiveTab] = useState<"write" | "preview">("write");
  const [sideBySide, setSideBySide] = useState(false);
  const [debouncedText, setDebouncedText] = useState("");
  const [saving, setSaving] = useState(false);
  const [prompting, setPrompting] = useState(false);
  const [pendingImprovement, setPendingImprovement] =
    useState<PendingImprovement | null>(null);

  // Plan-10: repository selector — only shown when starting a workflow.
  const [repos, setRepos] = useState<RepositoryRow[]>([]);
  const [repositoryId, setRepositoryId] = useState("");
  const [loadingRepos, setLoadingRepos] = useState(showStartButton);

  useEffect(() => {
    if (!showStartButton) return;
    setLoadingRepos(true);
    listMyRepositories()
      .then((rs) => {
        setRepos(rs);
        if (rs.length > 0) setRepositoryId(rs[0].id);
      })
      .catch(() => setRepos([]))
      .finally(() => setLoadingRepos(false));
  }, [showStartButton]);

  // Debounce editText for the side-by-side preview pane (400 ms)
  useEffect(() => {
    const id = setTimeout(() => setDebouncedText(editText), 400);
    return () => clearTimeout(id);
  }, [editText]);

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
    setEditText(improvedDescription);
    setDebouncedText(improvedDescription);
    if (improvedSummary) setEditTitle(improvedSummary);
    setPendingImprovement(null);
    if (!editMode) {
      setActiveTab("write");
      setSideBySide(false);
      setEditMode(true);
    }
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

  const handleStartEdit = () => {
    setEditText(markdown);
    setEditTitle(summary);
    setDebouncedText(markdown);
    setActiveTab("write");
    setSideBySide(false);
    setEditMode(true);
  };

  const handleSaveDescription = async () => {
    setSaving(true);
    try {
      const payload: Record<string, string> = { description: editText };
      if (editTitle !== summary) {
        payload.summary = editTitle;
      }
      const res = await apiPost(`/api/tickets/${encodeURIComponent(ticketKey)}/update-description`, payload);
      if (!res.ok) {
        const text = await res.text();
        throw new Error(text || `HTTP ${res.status}`);
      }
      setMarkdown(editText);
      requestAnimationFrame(() => {
        setEditMode(false);
        setActiveTab("write");
        setSideBySide(false);
      });
      onSaved?.();
    } catch (e) {
      showToast(e instanceof Error ? e.message : "Failed to save");
    } finally {
      setSaving(false);
    }
  };

  const handleCancelEdit = () => {
    setEditMode(false);
    setActiveTab("write");
    setSideBySide(false);
  };

  const handleSideBySideChange = (checked: boolean) => {
    setSideBySide(checked);
    if (!checked) setActiveTab("write");
  };

  const editDirty = editMode && (editText !== markdown || editTitle !== summary);

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
        {showStartButton && (
          <div className="px-4 py-3 border-b border-gray-800 flex items-center gap-3">
            <label className="text-xs text-gray-400 shrink-0">Repository:</label>
            {loadingRepos ? (
              <span className="text-xs text-gray-500">Loading…</span>
            ) : repos.length === 0 ? (
              <span className="text-xs text-amber-300">
                No repositories on your dashboard.{" "}
                <Link
                  to="/config.html?tab=repositories"
                  className="underline hover:text-amber-100"
                  onClick={onClose}
                >
                  Add one
                </Link>{" "}
                before starting a workflow.
              </span>
            ) : repos.length === 1 ? (
              <span className="text-xs text-gray-300 font-mono">{repos[0].name}</span>
            ) : (
              <select
                value={repositoryId}
                onChange={(e) => setRepositoryId(e.target.value)}
                className="bg-gray-950 border border-gray-700 rounded-lg px-2 py-1 text-xs text-gray-200 font-mono"
              >
                {repos.map((r) => (
                  <option key={r.id} value={r.id}>
                    {r.name}
                  </option>
                ))}
              </select>
            )}
          </div>
        )}

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
        {editMode && !pendingImprovement && (
          <div className={`border-b px-4 py-2 flex items-center justify-between ${editDirty ? "bg-blue-900/20 border-blue-700/30" : "bg-gray-800/30 border-gray-800"}`}>
            <span className={`text-xs ${editDirty ? "text-blue-300" : "text-gray-500"}`}>
              {editDirty ? "Description modified — save to update the ticket" : "Editing description"}
            </span>
            <div className="flex gap-2">
              <button
                onClick={handleCancelEdit}
                className="text-xs px-3 py-1 rounded-lg bg-gray-800 text-gray-300 border border-gray-700 hover:bg-gray-700 cursor-pointer"
              >
                Discard
              </button>
              <button
                onClick={handleSaveDescription}
                disabled={saving || !editDirty}
                className="text-xs px-3 py-1 rounded-lg bg-blue-600 text-white hover:bg-blue-500 disabled:opacity-50 cursor-pointer"
              >
                {saving ? "Saving..." : "Save"}
              </button>
            </div>
          </div>
        )}

        {/* Tab bar — only visible in edit mode and not while reviewing a diff */}
        {editMode && !pendingImprovement && (
          <div className="flex items-center px-6 py-2 border-b border-gray-800 gap-2">
            {!sideBySide && (
              <div className="flex gap-1">
                <button
                  onClick={() => setActiveTab("write")}
                  className={`text-xs px-3 py-1.5 rounded-md cursor-pointer ${
                    activeTab === "write"
                      ? "bg-gray-800 text-gray-200 border border-gray-700"
                      : "text-gray-500 hover:text-gray-300"
                  }`}
                >
                  Write
                </button>
                <button
                  onClick={() => setActiveTab("preview")}
                  className={`text-xs px-3 py-1.5 rounded-md cursor-pointer ${
                    activeTab === "preview"
                      ? "bg-gray-800 text-gray-200 border border-gray-700"
                      : "text-gray-500 hover:text-gray-300"
                  }`}
                >
                  Preview
                </button>
              </div>
            )}
            <label className="ml-auto flex items-center gap-2 text-xs text-gray-400 cursor-pointer select-none">
              <input
                type="checkbox"
                checked={sideBySide}
                onChange={(e) => handleSideBySideChange(e.target.checked)}
                className="w-3 h-3 accent-blue-500"
              />
              Side by side
            </label>
          </div>
        )}

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
            sideBySide ? (
              <>
                <div className="flex-1 overflow-y-auto p-4 border-r border-gray-800">
                  <textarea
                    value={editText}
                    onChange={(e) => setEditText(e.target.value)}
                    className="w-full h-full min-h-64 bg-gray-950 border border-gray-700 rounded-lg p-3 text-sm text-gray-200 font-mono resize-none"
                    autoFocus
                  />
                </div>
                <div className="flex-1 overflow-y-auto p-4">
                  <MarkdownPreview markdown={debouncedText} />
                </div>
              </>
            ) : activeTab === "write" ? (
              <div className="flex-1 flex flex-col p-6 min-h-0">
                <textarea
                  value={editText}
                  onChange={(e) => setEditText(e.target.value)}
                  className="w-full flex-1 bg-gray-950 border border-gray-700 rounded-lg p-3 text-sm text-gray-200 font-mono resize-none"
                  autoFocus
                />
              </div>
            ) : (
              <div className="flex-1 overflow-y-auto p-6">
                <MarkdownPreview markdown={editText} />
              </div>
            )
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
          <div className="flex gap-2">
            <button
              onClick={
                pendingImprovement
                  ? handleDiscardImprovement
                  : editMode
                  ? handleCancelEdit
                  : onClose
              }
              className="text-xs px-4 py-1.5 rounded-lg bg-gray-800 text-gray-300 border border-gray-700 hover:bg-gray-700 cursor-pointer"
            >
              {pendingImprovement ? "Discard" : editMode ? "Cancel" : "Close"}
            </button>
            {pendingImprovement ? (
              <button
                onClick={handleConfirmImprovement}
                className="text-xs px-4 py-1.5 rounded-lg bg-green-700 text-white hover:bg-green-600 cursor-pointer"
              >
                Confirm
              </button>
            ) : showStartButton && onStart ? (
              <button
                onClick={() => onStart(editMode ? editText : markdown, editMode ? editTitle : summary, repositoryId)}
                disabled={!repositoryId || loadingRepos}
                className="text-xs px-4 py-1.5 rounded-lg bg-blue-600 text-white hover:bg-blue-500 disabled:opacity-50 disabled:cursor-not-allowed cursor-pointer"
              >
                Add to Dashboard
              </button>
            ) : null}
          </div>
        </div>
      </div>
    </div>
  );
}
