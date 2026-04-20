import { useState, useEffect, useRef, useCallback } from "react";
import { marked } from "marked";
import DOMPurify from "dompurify";
import { apiJson, apiPost } from "../../api/client";
import type { TicketPreview, ImproveResponse } from "../../api/types";

const IMPROVE_TIMEOUT_SECS = 300;

interface Props {
  ticketKey: string;
  summary: string;
  description?: string;
  ticketingSystem: string;
  showStartButton: boolean;
  onStart?: () => void;
  onClose: () => void;
}

function formatCountdown(secs: number): string {
  const m = Math.floor(secs / 60);
  const s = secs % 60;
  return `${m}:${String(s).padStart(2, "0")} remaining until timeout`;
}

export function TicketDetailModal({
  ticketKey,
  summary,
  description: initialDescription,
  ticketingSystem,
  showStartButton,
  onStart,
  onClose,
}: Props) {
  const [markdown, setMarkdown] = useState(initialDescription || "");
  const [loading, setLoading] = useState(!initialDescription);
  const [improving, setImproving] = useState(false);
  const [countdown, setCountdown] = useState(IMPROVE_TIMEOUT_SECS);
  const [editMode, setEditMode] = useState(false);
  const [editText, setEditText] = useState("");
  const [originalMarkdown, setOriginalMarkdown] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  const abortRef = useRef<AbortController | null>(null);
  const countdownRef = useRef<ReturnType<typeof setInterval> | null>(null);

  // Cleanup on unmount
  useEffect(() => {
    return () => {
      abortRef.current?.abort();
      if (countdownRef.current) clearInterval(countdownRef.current);
    };
  }, []);

  useEffect(() => {
    if (initialDescription) return;
    if (ticketingSystem === "github") return;
    apiJson<TicketPreview>(`/api/jira/tickets/${encodeURIComponent(ticketKey)}/preview`)
      .then((data) => setMarkdown(data.description_markdown || ""))
      .catch(() => setMarkdown("*Failed to load description*"))
      .finally(() => setLoading(false));
  }, [ticketKey, initialDescription, ticketingSystem]);

  const renderHtml = useCallback(() => {
    const raw = marked.parse(markdown) as string;
    return DOMPurify.sanitize(raw);
  }, [markdown]);

  const handleImprove = async () => {
    setImproving(true);
    setCountdown(IMPROVE_TIMEOUT_SECS);

    // Start countdown timer
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
        body: JSON.stringify({ description: markdown, summary }),
        signal: abortRef.current.signal,
      });
      abortRef.current = null;
      if (!res.ok) {
        const text = await res.text();
        alert(text || "Failed to improve ticket description");
        return;
      }
      const data: ImproveResponse = await res.json();
      setOriginalMarkdown(markdown);
      setMarkdown(data.improved_description);
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

  const handleRevert = () => {
    if (originalMarkdown !== null) {
      setMarkdown(originalMarkdown);
      setOriginalMarkdown(null);
    }
  };

  const handleSaveImproved = async () => {
    setSaving(true);
    try {
      await apiPost(`/api/tickets/${encodeURIComponent(ticketKey)}/update-description`, {
        description: markdown,
      });
      setOriginalMarkdown(null);
    } catch (e) {
      alert(e instanceof Error ? e.message : "Failed to save");
    } finally {
      setSaving(false);
    }
  };

  const handleSaveEdit = async () => {
    try {
      await apiPost(`/api/tickets/${encodeURIComponent(ticketKey)}/update-description`, {
        description: editText,
      });
      setMarkdown(editText);
      setEditMode(false);
      setOriginalMarkdown(null);
    } catch (e) {
      alert(e instanceof Error ? e.message : "Failed to save");
    }
  };

  const isImproved = originalMarkdown !== null;

  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div
        className="bg-gray-900 border border-gray-700 rounded-xl w-full mx-4 max-h-[90vh] flex flex-col relative"
        style={{ maxWidth: "min(1280px, calc(100vw - 24px))" }}
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

        <div className="flex items-center justify-between p-4 border-b border-gray-800">
          <div className="min-w-0">
            <span className="font-mono text-xs text-blue-400">{ticketKey}</span>
            <h3 className="text-lg font-medium text-white truncate">{summary}</h3>
          </div>
          <button onClick={onClose} className="text-gray-500 hover:text-gray-300 cursor-pointer text-xl flex-shrink-0 ml-4">
            &times;
          </button>
        </div>

        {isImproved && (
          <div className="bg-purple-900/20 border-b border-purple-700/30 px-4 py-2 flex items-center justify-between">
            <span className="text-xs text-purple-300">AI-improved description — review before saving</span>
            <div className="flex gap-2">
              <button
                onClick={handleRevert}
                className="text-xs px-3 py-1 rounded-lg bg-gray-800 text-gray-300 border border-gray-700 hover:bg-gray-700 cursor-pointer"
              >
                Revert
              </button>
              <button
                onClick={handleSaveImproved}
                disabled={saving}
                className="text-xs px-3 py-1 rounded-lg bg-purple-600 text-white hover:bg-purple-500 disabled:opacity-50 cursor-pointer"
              >
                {saving ? "Saving..." : "Save"}
              </button>
            </div>
          </div>
        )}

        <div className="overflow-y-auto flex-1 p-6">
          {loading ? (
            <p className="text-gray-500 text-sm">Loading description...</p>
          ) : editMode ? (
            <textarea
              value={editText}
              onChange={(e) => setEditText(e.target.value)}
              className="w-full h-64 bg-gray-950 border border-gray-700 rounded-lg p-3 text-sm text-gray-200 font-mono resize-y"
            />
          ) : (
            <div
              className="prose prose-invert prose-sm max-w-none"
              dangerouslySetInnerHTML={{ __html: renderHtml() }}
            />
          )}
        </div>

        <div className="flex items-center justify-between p-4 border-t border-gray-800 gap-3">
          <div className="flex gap-2">
            {!isImproved && (
              <button
                onClick={handleImprove}
                disabled={improving || editMode}
                className="text-xs px-3 py-1.5 rounded-lg bg-purple-600/20 text-purple-300 border border-purple-500/30 hover:bg-purple-600/30 disabled:opacity-50 cursor-pointer"
              >
                Improve with AI
              </button>
            )}
            {!editMode && !isImproved ? (
              <button
                onClick={() => {
                  setEditText(markdown);
                  setEditMode(true);
                }}
                className="text-xs px-3 py-1.5 rounded-lg bg-gray-800 text-gray-300 border border-gray-700 hover:bg-gray-700 cursor-pointer"
              >
                Edit
              </button>
            ) : editMode ? (
              <>
                <button
                  onClick={handleSaveEdit}
                  className="text-xs px-3 py-1.5 rounded-lg bg-green-600/20 text-green-300 border border-green-500/30 hover:bg-green-600/30 cursor-pointer"
                >
                  Save
                </button>
                <button
                  onClick={() => setEditMode(false)}
                  className="text-xs px-3 py-1.5 rounded-lg bg-gray-800 text-gray-300 border border-gray-700 hover:bg-gray-700 cursor-pointer"
                >
                  Cancel
                </button>
              </>
            ) : null}
          </div>
          <div className="flex gap-2">
            <button
              onClick={onClose}
              className="text-xs px-4 py-1.5 rounded-lg bg-gray-800 text-gray-300 border border-gray-700 hover:bg-gray-700 cursor-pointer"
            >
              Close
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
