import { useState, useEffect } from "react";
import { apiJson, apiPostJson, apiPost } from "../../api/client";
import type { TicketPreview, ImproveResponse } from "../../api/types";
import { MarkdownPreview } from "../MarkdownPreview";

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
  const [activeTab, setActiveTab] = useState<"write" | "preview">("write");
  const [sideBySide, setSideBySide] = useState(false);
  const [debouncedText, setDebouncedText] = useState("");
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    if (initialDescription) return;
    if (ticketingSystem === "github") return;
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

  const handleImprove = async () => {
    setImproving(true);
    try {
      const data = await apiPostJson<ImproveResponse>(
        `/api/tickets/${encodeURIComponent(ticketKey)}/improve`,
        { description: markdown, summary }
      );
      setMarkdown(data.improved_description);
    } catch (e) {
      alert(e instanceof Error ? e.message : "Failed to improve");
    } finally {
      setImproving(false);
    }
  };

  const handleStartEdit = () => {
    setEditText(markdown);
    setDebouncedText(markdown);
    setActiveTab("write");
    setSideBySide(false);
    setEditMode(true);
  };

  const handleSaveDescription = async () => {
    setSaving(true);
    try {
      const res = await apiPost(`/api/tickets/${encodeURIComponent(ticketKey)}/update-description`, {
        description: editText,
      });
      if (!res.ok) {
        const text = await res.text();
        throw new Error(text || `HTTP ${res.status}`);
      }
      setMarkdown(editText);
      setEditMode(false);
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

  const editDirty = editMode && editText !== markdown;

  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div
        className="bg-gray-900 border border-gray-700 rounded-xl w-full mx-4 flex flex-col"
        style={{ maxWidth: "min(1280px, calc(100vw - 24px))", height: "calc(100vh - 48px)" }}
        onClick={(e) => e.stopPropagation()}
      >
        {/* Header */}
        <div className="flex items-center justify-between p-4 border-b border-gray-800">
          <div className="min-w-0">
            <span className="font-mono text-xs text-blue-400">{ticketKey}</span>
            <h3 className="text-lg font-medium text-white truncate">{summary}</h3>
          </div>
          <button onClick={onClose} className="text-gray-500 hover:text-gray-300 cursor-pointer text-xl flex-shrink-0 ml-4">
            &times;
          </button>
        </div>

        {/* Modified banner — shown when editing and text differs, or after AI improve */}
        {editDirty && (
          <div className="bg-blue-900/20 border-b border-blue-700/30 px-4 py-2 flex items-center justify-between">
            <span className="text-xs text-blue-300">Description modified — save to update the ticket</span>
            <div className="flex gap-2">
              <button
                onClick={handleCancelEdit}
                className="text-xs px-3 py-1 rounded-lg bg-gray-800 text-gray-300 border border-gray-700 hover:bg-gray-700 cursor-pointer"
              >
                Discard
              </button>
              <button
                onClick={handleSaveDescription}
                disabled={saving}
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
        <div className={`flex-1 ${editMode && sideBySide ? "flex overflow-hidden" : "flex flex-col overflow-hidden"}`}>
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
              <div className="flex-1 overflow-y-auto p-6">
                <textarea
                  value={editText}
                  onChange={(e) => setEditText(e.target.value)}
                  className="w-full h-64 bg-gray-950 border border-gray-700 rounded-lg p-3 text-sm text-gray-200 font-mono resize-y"
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
