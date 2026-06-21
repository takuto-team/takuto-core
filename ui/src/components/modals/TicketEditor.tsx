// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * `TicketEditor` — namespace of three composable JSX slices used by
 * `TicketDetailModal` for description editing: Banner (dirty/save row),
 * TabBar (Write/Preview tabs + side-by-side toggle), and Content
 * (textarea + preview). State + handlers live in `useTicketEditor`.
 */

import { useTranslation } from "react-i18next";
import { MarkdownPreview } from "../MarkdownPreview";

interface BannerProps {
  editMode: boolean;
  pendingImprovement: unknown;
  editDirty: boolean;
  saving: boolean;
  onCancelEdit: () => void;
  onSaveDescription: () => void;
}

function TicketEditorBanner({
  editMode, pendingImprovement, editDirty, saving, onCancelEdit, onSaveDescription,
}: BannerProps) {
  const { t } = useTranslation("modals");
  if (!editMode || pendingImprovement) return null;
  return (
    <div className={`border-b px-4 py-2 flex items-center justify-between ${editDirty ? "bg-blue-900/20 border-blue-700/30" : "bg-gray-800/30 border-gray-800"}`}>
      <span className={`text-xs ${editDirty ? "text-blue-300" : "text-gray-500"}`}>
        {editDirty ? t("ticketEditor.descriptionModified") : t("ticketEditor.editingDescription")}
      </span>
      <div className="flex gap-2">
        <button
          onClick={onCancelEdit}
          className="text-xs px-3 py-1 rounded-lg bg-gray-800 text-gray-300 border border-gray-700 hover:bg-gray-700 cursor-pointer"
        >
          {t("ticketEditor.discard")}
        </button>
        <button
          onClick={onSaveDescription}
          disabled={saving || !editDirty}
          className="text-xs px-3 py-1 rounded-lg bg-blue-600 text-white hover:bg-blue-500 disabled:opacity-50 cursor-pointer"
        >
          {saving ? t("ticketEditor.saving") : t("ticketEditor.save")}
        </button>
      </div>
    </div>
  );
}

interface TabBarProps {
  editMode: boolean;
  pendingImprovement: unknown;
  sideBySide: boolean;
  activeTab: "write" | "preview";
  setActiveTab: (next: "write" | "preview") => void;
  onSideBySideChange: (checked: boolean) => void;
}

function TicketEditorTabBar({
  editMode, pendingImprovement, sideBySide, activeTab, setActiveTab, onSideBySideChange,
}: TabBarProps) {
  const { t } = useTranslation("modals");
  if (!editMode || pendingImprovement) return null;
  const writeClass = activeTab === "write"
    ? "bg-gray-800 text-gray-200 border border-gray-700"
    : "text-gray-500 hover:text-gray-300";
  const previewClass = activeTab === "preview"
    ? "bg-gray-800 text-gray-200 border border-gray-700"
    : "text-gray-500 hover:text-gray-300";
  return (
    <div className="flex items-center px-6 py-2 border-b border-gray-800 gap-2">
      {!sideBySide && (
        <div className="flex gap-1">
          <button onClick={() => setActiveTab("write")} className={`text-xs px-3 py-1.5 rounded-md cursor-pointer ${writeClass}`}>{t("ticketEditor.write")}</button>
          <button onClick={() => setActiveTab("preview")} className={`text-xs px-3 py-1.5 rounded-md cursor-pointer ${previewClass}`}>{t("ticketEditor.preview")}</button>
        </div>
      )}
      <label className="ml-auto flex items-center gap-2 text-xs text-gray-400 cursor-pointer select-none">
        <input
          type="checkbox"
          checked={sideBySide}
          onChange={(e) => onSideBySideChange(e.target.checked)}
          className="w-3 h-3 accent-blue-500"
        />
        {t("ticketEditor.sideBySide")}
      </label>
    </div>
  );
}

interface ContentProps {
  sideBySide: boolean;
  activeTab: "write" | "preview";
  editText: string;
  setEditText: (next: string) => void;
  debouncedText: string;
}

function TicketEditorContent({ sideBySide, activeTab, editText, setEditText, debouncedText }: ContentProps) {
  if (sideBySide) {
    return (
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
    );
  }
  if (activeTab === "write") {
    return (
      <div className="flex-1 flex flex-col p-6 min-h-0">
        <textarea
          value={editText}
          onChange={(e) => setEditText(e.target.value)}
          className="w-full flex-1 bg-gray-950 border border-gray-700 rounded-lg p-3 text-sm text-gray-200 font-mono resize-none"
          autoFocus
        />
      </div>
    );
  }
  return (
    <div className="flex-1 overflow-y-auto p-6">
      <MarkdownPreview markdown={editText} />
    </div>
  );
}

export const TicketEditor = {
  Banner: TicketEditorBanner,
  TabBar: TicketEditorTabBar,
  Content: TicketEditorContent,
};
