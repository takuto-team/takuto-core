import { useState, useEffect, useRef } from "react";
import { apiJson, apiPost } from "../../api/client";
import type { TicketPreview, ImproveResponse } from "../../api/types";
import { MarkdownPreview } from "../MarkdownPreview";

const IMPROVE_TIMEOUT_SECS = 300;

function formatCountdown(secs: number): string {
  const m = Math.floor(secs / 60);
  const s = secs % 60;
  return `${m}:${String(s).padStart(2, "0")} remaining until timeout`;
}

interface Props {
  ticketKey: string;
  summary: string;
  description?: string;
  ticketingSystem: string;
  showStartButton: boolean;
  onStart?: () => void;
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
  onStart,
  onClose,
  onSaved,
}: Props) {
  const [markdown, setMarkdown] = useState(initialDescription || "");
  const [loading, setLoading] = useState(!initialDescription && ticketingSystem !== "none");
  const [editTitle, setEditTitle] = useState(summary);
  const [improving, setImproving] = useState(false);
  const [countdown, setCountdown] = useState(IMPROVE_TIMEOUT_SECS);
  const abortRef = useRef<AbortController | null>(null);
  const countdownRef = useRef<ReturnType<typeof setInterval> | null>(null);
  const [editMode, setEditMode] = useState(false);
  const [editText, setEditText] = useState("");
  const [activeTab, setActiveTab] = useState<"write" | "preview">("write");
  const [sideBySide, setSideBySide] = useState(false);
  const [debouncedText, setDebouncedText] = useState("");
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    // No-ticketing mode: description comes from the workflow (initialDescription prop).
    // GitHub mode: description comes from the workflow (cached issue body).
    // Only Jira mode needs to fetch from the preview API.
    if (initialDescription || ticketingSystem === "none" || ticketingSystem === "github") return;
    apiJson<TicketPreview>(`/api/jira/tickets/${encodeURIComponent(ticketKey)}/preview`)
      .then((data) => setMarkdown(data.description_markdown || ""))
      .catch(() => setMarkdown("*Failed to load description*"))
      .finally(() => setLoading(false));
  }, [ticketKey, initialDescription, ticketingSystem]);

  // Debounce editText for the side-by-side preview pane (400 ms)
  useEffect(() => {
    const id = setTimeout(() => setDebouncedText(editText), 400);
    return () => clearTimeout(id);
  }, [editText]);

  // Cleanup abort/countdown on unmount
  useEffect(() => {
    return () => {
      abortRef.current?.abort();
      if (countdownRef.current) clearInterval(countdownRef.current);
    };
  }, []);

  const handleImprove = async () => {
    setImproving(true);
    setCountdown(IMPROVE_TIMEOUT_SECS);

    if (countdownRef.current) clearInterval(countdownRef.current);
    countdownRef.current = setInterval(() => {
      setCountdown((prev) => Math.max(0, prev - 1));
    }, 1000);

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
        alert(text || "Failed to improve ticket description");
        return;
      }
      const data: ImproveResponse = await res.json();
      setMarkdown(data.improved_description);
      if (data.improved_summary) {
        setEditTitle(data.improved_summary);
      }
      if (!editMode) {
        setEditText(data.improved_description);
        setDebouncedText(data.improved_description);
        setEditMode(true);
      }
    } catch (e) {
      abortRef.current = null;
      if (e instanceof Error && e.name !== "AbortError") {
        alert("Failed to improve ticket description");
      }
    } finally {
      setImproving(false);
      if (countdownRef.current) {
        clearInterval(countdownRef.current);
        countdownRef.current = null;
      }
    }
  };

  const handleCancelImprove = () => {
    abortRef.current?.abort();
    abortRef.current = null;
    setImproving(false);
    if (countdownRef.current) {
      clearInterval(countdownRef.current);
      countdownRef.current = null;
    }
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
      alert(e instanceof Error ? e.message : "Failed to save");
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

  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div
        className="bg-gray-900 border border-gray-700 rounded-xl w-full mx-4 flex flex-col relative"
        style={{ maxWidth: "min(1280px, calc(100vw - 24px))", height: "calc(100vh - 48px)" }}
        onClick={(e) => e.stopPropagation()}
      >
        {/* Improve overlay with countdown */}
        {improving && (
          <div className="absolute inset-0 z-10 flex flex-col items-center justify-center bg-gray-900/85 backdrop-blur-sm rounded-xl">
            <div className="w-8 h-8 border-2 border-gray-600 border-t-blue-400 rounded-full animate-spin" />
            <p className="mt-4 text-sm text-gray-300">Improving description...</p>
            <p className="mt-1 text-xs text-gray-500">{formatCountdown(countdown)}</p>
            <button
              onClick={handleCancelImprove}
              className="mt-4 text-xs px-3 py-1.5 rounded-lg bg-gray-800 text-gray-300 border border-gray-700 hover:bg-gray-700 cursor-pointer"
            >
              Cancel
            </button>
          </div>
        )}

        {/* Header */}
        <div className="flex items-center justify-between p-4 border-b border-gray-800">
          <div className="min-w-0 flex-1">
            <span className="font-mono text-xs text-blue-400">{ticketKey}</span>
            {editMode ? (
              <input
                type="text"
                value={editTitle}
                onChange={(e) => setEditTitle(e.target.value)}
                className="block w-full mt-1 bg-gray-950 border border-gray-700 rounded-lg px-3 py-1.5 text-lg font-medium text-white"
              />
            ) : (
              <h3 className="text-lg font-medium text-white truncate">{summary}</h3>
            )}
          </div>
          <button onClick={onClose} className="text-gray-500 hover:text-gray-300 cursor-pointer text-xl flex-shrink-0 ml-4">
            &times;
          </button>
        </div>

        {/* Edit banner — always visible in edit mode; buttons disabled until content changes */}
        {editMode && (
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

        {/* Tab bar — only visible in edit mode */}
        {editMode && (
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
        <div className={`flex-1 min-h-0 ${editMode && sideBySide ? "flex overflow-hidden" : "flex flex-col overflow-hidden"}`}>
          {loading ? (
            <div className="flex-1 overflow-y-auto p-6">
              <p className="text-gray-500 text-sm">Loading description...</p>
            </div>
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
            <div className="flex-1 overflow-y-auto p-6">
              <MarkdownPreview markdown={markdown} />
            </div>
          )}
        </div>

        {/* Footer */}
        <div className="flex items-center justify-between p-4 border-t border-gray-800 gap-3">
          <div className="flex gap-2">
            <button
              onClick={handleImprove}
              disabled={improving || editMode}
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
          </div>
          <div className="flex gap-2">
            <button
              onClick={editMode ? handleCancelEdit : onClose}
              className="text-xs px-4 py-1.5 rounded-lg bg-gray-800 text-gray-300 border border-gray-700 hover:bg-gray-700 cursor-pointer"
            >
              {editMode ? "Cancel" : "Close"}
            </button>
            {showStartButton && onStart && (
              <button
                onClick={onStart}
                className="text-xs px-4 py-1.5 rounded-lg bg-blue-600 text-white hover:bg-blue-500 cursor-pointer"
              >
                Start
              </button>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}
