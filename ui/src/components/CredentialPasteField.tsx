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

import { useId, useState, type ReactNode } from "react";

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
}

export function CredentialPasteField({
  label,
  value,
  onChange,
  onSubmit,
  helper,
  placeholder,
  saving = false,
  saveLabel = "Save",
}: Props) {
  const inputId = useId();
  const [revealed, setRevealed] = useState(false);
  const canSubmit = !saving && value.trim().length > 0;

  return (
    <div className="flex flex-col gap-2">
      <label htmlFor={inputId} className="text-xs text-gray-400">
        {label}
      </label>
      <div className="flex gap-2">
        <div className="relative flex-1">
          <input
            id={inputId}
            type={revealed ? "text" : "password"}
            value={value}
            onChange={(e) => onChange(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && canSubmit) onSubmit();
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
            aria-label={revealed ? "Hide credential" : "Show credential"}
            className="absolute right-2 top-1/2 -translate-y-1/2 text-xs text-gray-500 hover:text-gray-300 cursor-pointer"
          >
            {revealed ? "Hide" : "Show"}
          </button>
        </div>
        <button
          type="button"
          disabled={!canSubmit}
          onClick={onSubmit}
          className="text-sm px-4 py-2 rounded-lg bg-blue-600 text-white hover:bg-blue-500 disabled:opacity-50 disabled:cursor-not-allowed cursor-pointer"
        >
          {saving ? "Saving…" : saveLabel}
        </button>
      </div>
      {helper && <div className="text-xs text-gray-500">{helper}</div>}
    </div>
  );
}
