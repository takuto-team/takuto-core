// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Paste field — shared between the Cursor / Claude / GitHub-PAT panels.
 * Renders a password-style input with a show/hide eye toggle, an
 * optional helper line, and a Save button. The parent owns the controlled
 * value and the submit handler; this component holds only the local
 * "revealed" toggle.
 *
 * Security notes (mirrors 04_architecture.md §3 + CODING_STANDARDS §4):
 *   - The pasted value is never logged or written to localStorage.
 *   - The Save handler is fire-and-forget from the field's perspective —
 *     parent renders any error toast.
 */

import { useEffect, useId, useRef, useState, type ReactNode } from "react";
import { useTranslation } from "react-i18next";

interface Props {
  /** Label rendered above the input (e.g. "API key", "Personal access token"). */
  label: string;
  /** Current input value (controlled). */
  value: string;
  /** Called on every keystroke. */
  onChange: (next: string) => void;
  /** Called when the user clicks Save (or presses Enter inside the field). */
  onSubmit: () => void;
  /** Optional helper text rendered as a paragraph below the field. */
  helper?: ReactNode;
  /** Optional placeholder text inside the input. */
  placeholder?: string;
  /** Save-in-flight toggle — disables the input + button. */
  saving?: boolean;
  /** Save button copy. Defaults to "Save". */
  saveLabel?: string;
  /**
   * When true, render a danger-styled Delete button next to Save that wipes
   * the stored credential for the current provider (api_key slot). Hidden
   * entirely when false/undefined — there is nothing to delete.
   */
  canDelete?: boolean;
  /**
   * Called when the user confirms the delete (second click of the two-step
   * inline confirm). Omitting it hides the Delete button regardless of
   * `canDelete`.
   */
  onDelete?: () => void;
  /** Delete-in-flight toggle — disables the delete button while the request runs. */
  deleting?: boolean;
  /**
   * When true, hide the Save button and disable Enter-to-submit — the value is
   * persisted by a single page-level Save (the wizard / settings footer) which
   * calls the parent panel's deferred save. The Delete button still renders
   * when `canDelete`. Defaults to false (the field owns its own Save).
   */
  hideSave?: boolean;
  /**
   * When true, the secret is already stored on the server. The field renders a
   * fixed masked indicator (`••••••`, read-only) plus a "Replace" affordance
   * instead of an empty input — signalling "set" without ever holding or
   * sending the real secret. Clicking Replace enters edit mode (empty input);
   * leaving it masked/untouched means KEEP (the parent omits the secret on the
   * wire). Defaults to false (first-time entry).
   */
  masked?: boolean;
}

/** Fixed "secret is set" indicator. Display-only — never sent on the wire. */
const SECRET_MASK = "••••••";

export function CredentialPasteField({
  label,
  value,
  onChange,
  onSubmit,
  helper,
  placeholder,
  saving = false,
  saveLabel,
  canDelete = false,
  onDelete,
  deleting = false,
  hideSave = false,
  masked = false,
}: Props) {
  const { t } = useTranslation("credentials");
  const inputId = useId();
  const [revealed, setRevealed] = useState(false);
  // When the secret is stored, start in the masked "untouched" state until the
  // user clicks Replace. Re-mask whenever the stored state flips back to set
  // (e.g. after a successful save clears `value` and `masked` stays true).
  const [editing, setEditing] = useState(false);
  const showMask = masked && !editing && value.length === 0;
  // Two-click inline confirm for delete: first click arms ("Confirm"),
  // second click fires. Auto-disarms after a few seconds so a stray first
  // click never leaves the field primed to wipe on the next stray click.
  const [confirmingDelete, setConfirmingDelete] = useState(false);
  const disarmTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const showDelete = canDelete && !!onDelete;
  const canSubmit = !saving && !deleting && value.trim().length > 0;

  useEffect(() => {
    return () => {
      if (disarmTimer.current) clearTimeout(disarmTimer.current);
    };
  }, []);

  // Re-mask after a successful rotate: the parent clears `value` to "" on save
  // while the secret stays set (`masked` true), so drop back to the indicator.
  const prevValue = useRef(value);
  useEffect(() => {
    if (prevValue.current !== "" && value === "" && masked) setEditing(false);
    prevValue.current = value;
  }, [value, masked]);

  const handleDeleteClick = () => {
    if (!onDelete) return;
    if (!confirmingDelete) {
      setConfirmingDelete(true);
      if (disarmTimer.current) clearTimeout(disarmTimer.current);
      disarmTimer.current = setTimeout(() => setConfirmingDelete(false), 4000);
      return;
    }
    if (disarmTimer.current) clearTimeout(disarmTimer.current);
    setConfirmingDelete(false);
    onDelete();
  };

  return (
    <div className="flex flex-col gap-2">
      <label htmlFor={inputId} className="text-xs text-gray-400">
        {label}
      </label>
      <div className="flex gap-2">
        {showMask ? (
          <div className="flex-1 flex items-center gap-2">
            <input
              id={inputId}
              type="text"
              value={SECRET_MASK}
              readOnly
              aria-label={label}
              tabIndex={-1}
              className="w-full bg-gray-950 border border-gray-700 rounded-lg px-3 py-2 text-sm text-gray-500 font-mono"
            />
            <button
              type="button"
              onClick={() => setEditing(true)}
              className="text-sm px-4 py-2 rounded-lg bg-gray-800 text-gray-200 border border-gray-700 hover:bg-gray-700 cursor-pointer whitespace-nowrap"
            >
              {t("actions.replace")}
            </button>
          </div>
        ) : (
          <div className="relative flex-1">
            <input
              id={inputId}
              type={revealed ? "text" : "password"}
              value={value}
              onChange={(e) => onChange(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter" && canSubmit && !hideSave) onSubmit();
              }}
              placeholder={placeholder}
              disabled={saving}
              spellCheck={false}
              autoComplete="off"
              className="w-full bg-gray-950 border border-gray-700 rounded-lg px-3 py-2 text-sm text-gray-200 font-mono pr-10"
            />
            <button
              type="button"
              onClick={() => setRevealed((v) => !v)}
              aria-pressed={revealed}
              aria-label={revealed ? t("paste.concealAria") : t("paste.revealAria")}
              className="absolute right-2 top-1/2 -translate-y-1/2 text-xs text-gray-500 hover:text-gray-300 cursor-pointer"
            >
              {revealed ? t("paste.conceal") : t("paste.reveal")}
            </button>
          </div>
        )}
        {!hideSave && !showMask && (
          <button
            type="button"
            disabled={!canSubmit}
            onClick={onSubmit}
            className="text-sm px-4 py-2 rounded-lg bg-blue-600 text-white hover:bg-blue-500 disabled:opacity-50 disabled:cursor-not-allowed cursor-pointer"
          >
            {saving ? t("actions.saving") : (saveLabel ?? t("actions.save"))}
          </button>
        )}
        {showDelete && (
          <button
            type="button"
            disabled={deleting || saving}
            onClick={handleDeleteClick}
            onBlur={() => {
              if (disarmTimer.current) clearTimeout(disarmTimer.current);
              setConfirmingDelete(false);
            }}
            aria-label={
              confirmingDelete
                ? t("paste.confirmDeleteAria", { label })
                : t("paste.deleteAria", { label })
            }
            className={`text-sm px-4 py-2 rounded-lg border disabled:opacity-50 disabled:cursor-not-allowed cursor-pointer ${
              confirmingDelete
                ? "bg-red-600 border-red-500 text-white hover:bg-red-500"
                : "bg-transparent border-red-700/60 text-red-300 hover:bg-red-950/40"
            }`}
          >
            {deleting
              ? t("actions.deleting")
              : confirmingDelete
                ? t("actions.confirm")
                : t("actions.delete")}
          </button>
        )}
      </div>
      {helper && <div className="text-xs text-gray-500">{helper}</div>}
    </div>
  );
}
