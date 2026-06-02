// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * `useTicketEditor` — owns the description-editing state for
 * `TicketDetailModal`: title, text, write/preview tabs, side-by-side
 * toggle, the debounced preview value, and the save lifecycle.
 *
 * Behaviour preserved verbatim from the pre-split modal:
 *   * 400 ms debounce on `editText → debouncedText` (no unmount cleanup
 *     beyond the per-keystroke clearTimeout — designer flagged as
 *     intentional).
 *   * `requestAnimationFrame` post-save sequencing.
 *   * `applyImprovement` flips into edit mode iff not already editing.
 */

import { useEffect, useState } from "react";
import { apiPost } from "../api/client";
import { useToast } from "./useToast";

interface UseTicketEditorParams {
  summary: string;
  markdown: string;
  setMarkdown: (next: string) => void;
  ticketKey: string;
  onSaved?: () => void;
}

export interface UseTicketEditorResult {
  editMode: boolean;
  editText: string;
  editTitle: string;
  activeTab: "write" | "preview";
  sideBySide: boolean;
  debouncedText: string;
  saving: boolean;
  editDirty: boolean;
  setEditText: (next: string) => void;
  setEditTitle: (next: string) => void;
  setActiveTab: (next: "write" | "preview") => void;
  handleStartEdit: () => void;
  handleCancelEdit: () => void;
  handleSaveDescription: () => Promise<void>;
  handleSideBySideChange: (checked: boolean) => void;
  /** Editor-side half of "confirm pending improvement". Called by the
   *  improve-with-AI hook when the user accepts the AI's diff. */
  applyImprovement: (improvedDescription: string, improvedSummary?: string) => void;
}

export function useTicketEditor(params: UseTicketEditorParams): UseTicketEditorResult {
  const { summary, markdown, setMarkdown, ticketKey, onSaved } = params;
  const { showToast } = useToast();
  const [editTitle, setEditTitle] = useState(summary);
  const [editMode, setEditMode] = useState(false);
  const [editText, setEditText] = useState("");
  const [activeTab, setActiveTab] = useState<"write" | "preview">("write");
  const [sideBySide, setSideBySide] = useState(false);
  const [debouncedText, setDebouncedText] = useState("");
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    const id = setTimeout(() => setDebouncedText(editText), 400);
    return () => clearTimeout(id);
  }, [editText]);

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

  const applyImprovement = (improvedDescription: string, improvedSummary?: string) => {
    setEditText(improvedDescription);
    setDebouncedText(improvedDescription);
    if (improvedSummary) setEditTitle(improvedSummary);
    if (!editMode) {
      setActiveTab("write");
      setSideBySide(false);
      setEditMode(true);
    }
  };

  const editDirty = editMode && (editText !== markdown || editTitle !== summary);

  return {
    editMode,
    editText,
    editTitle,
    activeTab,
    sideBySide,
    debouncedText,
    saving,
    editDirty,
    setEditText,
    setEditTitle,
    setActiveTab,
    handleStartEdit,
    handleCancelEdit,
    handleSaveDescription,
    handleSideBySideChange,
    applyImprovement,
  };
}
