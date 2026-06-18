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
  /** Repo the ticket belongs to — sent so a not-yet-added GitHub issue saves to
   * the right repo's issue (ignored once the ticket is a work item). */
  repository?: string | null;
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
  /** Persists the description; resolves `true` on success, `false` if the
   *  save failed (a toast is shown in that case). */
  handleSaveDescription: () => Promise<boolean>;
  handleSideBySideChange: (checked: boolean) => void;
  /** Editor-side half of "confirm pending improvement". Called by the
   *  improve-with-AI hook when the user accepts the AI's diff. */
  applyImprovement: (improvedDescription: string, improvedSummary?: string) => void;
}

export function useTicketEditor(params: UseTicketEditorParams): UseTicketEditorResult {
  const { summary, markdown, setMarkdown, ticketKey, repository, onSaved } = params;
  const { showToast } = useToast();
  const [editTitle, setEditTitle] = useState(summary);
  // Last-persisted title. Saving stays in edit mode, so dirtiness is measured
  // against what's been saved (not the original `summary` prop) — otherwise a
  // saved title change would keep reading as unsaved.
  const [savedSummary, setSavedSummary] = useState(summary);
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
    setEditTitle(savedSummary);
    setDebouncedText(markdown);
    setActiveTab("write");
    setSideBySide(false);
    setEditMode(true);
  };

  const handleSaveDescription = async (): Promise<boolean> => {
    setSaving(true);
    try {
      const payload: Record<string, string> = { description: editText };
      if (editTitle !== savedSummary) {
        payload.summary = editTitle;
      }
      if (repository) {
        payload.repository = repository;
      }
      const res = await apiPost(`/api/tickets/${encodeURIComponent(ticketKey)}/update-description`, payload);
      if (!res.ok) {
        const text = await res.text();
        throw new Error(text || `HTTP ${res.status}`);
      }
      // Persist only — stay in edit mode. Advancing both baselines clears
      // editDirty (so the Save button disables) without dismissing the editor.
      setMarkdown(editText);
      setSavedSummary(editTitle);
      onSaved?.();
      return true;
    } catch (e) {
      showToast(e instanceof Error ? e.message : "Failed to save");
      return false;
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

  const editDirty = editMode && (editText !== markdown || editTitle !== savedSummary);

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
