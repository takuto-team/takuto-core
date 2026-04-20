import { useState, useEffect } from "react";
import { marked } from "marked";
import DOMPurify from "dompurify";
import { apiJson, apiPostJson, apiPost } from "../../api/client";
import type { TicketPreview, ImproveResponse } from "../../api/types";

interface Props {
  ticketKey: string;
  summary: string;
  description?: string;
  ticketingSystem: string;
  showStartButton: boolean;
  onStart?: () => void;
  onClose: () => void;
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
  const [editMode, setEditMode] = useState(false);
  const [editText, setEditText] = useState("");
  /** Snapshot of markdown before AI improvement; null means not in improved state. */
  const [originalMarkdown, setOriginalMarkdown] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    if (initialDescription) return;
    if (ticketingSystem === "github") return;
    apiJson<TicketPreview>(`/api/jira/tickets/${encodeURIComponent(ticketKey)}/preview`)
      .then((data) => setMarkdown(data.description_markdown || ""))
      .catch(() => setMarkdown("*Failed to load description*"))
      .finally(() => setLoading(false));
  }, [ticketKey, initialDescription, ticketingSystem]);

  const renderHtml = () => {
    const raw = marked.parse(markdown) as string;
    return DOMPurify.sanitize(raw);
  };

  const handleImprove = async () => {
    setImproving(true);
    try {
      const data = await apiPostJson<ImproveResponse>(
        `/api/tickets/${encodeURIComponent(ticketKey)}/improve`,
        { description: markdown, summary }
      );
      setOriginalMarkdown(markdown);
      setMarkdown(data.improved_description);
    } catch (e) {
      alert(e instanceof Error ? e.message : "Failed to improve");
    } finally {
      setImproving(false);
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
        className="bg-gray-900 border border-gray-700 rounded-xl w-full mx-4 max-h-[90vh] flex flex-col"
        style={{ maxWidth: "min(1280px, calc(100vw - 24px))" }}
        onClick={(e) => e.stopPropagation()}
      >
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
                {improving ? "Improving..." : "Improve with AI"}
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
