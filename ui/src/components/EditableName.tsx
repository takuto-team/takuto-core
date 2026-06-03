// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { useEffect, useRef, useState } from "react";

interface EditableNameProps {
  value: string;
  onChange: (next: string) => void;
  /** Fired when the user finishes editing (blur or Enter). Use for inline persist. */
  onCommit?: (value: string) => void;
  placeholder: string;
  /** Tailwind classes for the rendered text / input (sizing, weight). */
  textClassName: string;
  /** Optional click-to-rename tooltip override. */
  title?: string;
}

/**
 * A name surface that is a clickable label in display mode and swaps to an
 * inline `<input>` on click. Used in the flow card row and in each step's
 * header so the user can rename in place without a separate field.
 *
 * Click events stop propagating so this can sit inside an outer button (the
 * card row that toggles expand/collapse) without triggering it.
 */
export function EditableName({
  value,
  onChange,
  onCommit,
  placeholder,
  textClassName,
  title,
}: EditableNameProps) {
  const [editing, setEditing] = useState(false);
  const inputRef = useRef<HTMLInputElement | null>(null);
  const escapingRef = useRef(false);
  const originalRef = useRef(value);

  useEffect(() => {
    if (editing) {
      originalRef.current = value;
      inputRef.current?.focus();
    }
    // Intentionally re-snapshot `value` only when entering edit mode so
    // mid-edit prop changes don't move the revert target. Escape always
    // restores the value the field had when the user clicked into it.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [editing]);

  if (editing) {
    return (
      <input
        ref={inputRef}
        type="text"
        value={value}
        onChange={(e) => onChange(e.target.value)}
        onBlur={() => {
          setEditing(false);
          if (escapingRef.current) {
            escapingRef.current = false;
            // Revert: parent value goes back to what it was at edit-start.
            if (value !== originalRef.current) onChange(originalRef.current);
            return;
          }
          if (onCommit) onCommit(value);
        }}
        onClick={(e) => e.stopPropagation()}
        onKeyDown={(e) => {
          // The input may sit inside a `<button>` (the collapsed flow row).
          // Native buttons activate on Space and Enter, so every keystroke
          // must be stopped from bubbling — otherwise typing a space would
          // toggle the card instead of inserting the space.
          e.stopPropagation();
          if (e.key === "Enter") {
            e.preventDefault();
            e.currentTarget.blur();
          } else if (e.key === "Escape") {
            e.preventDefault();
            escapingRef.current = true;
            e.currentTarget.blur();
          }
        }}
        onKeyUp={(e) => e.stopPropagation()}
        placeholder={placeholder}
        className={`${textClassName} min-w-0 bg-gray-950 border border-blue-500 rounded px-2 py-0.5 text-gray-200 focus:outline-none`}
      />
    );
  }

  return (
    <span
      role="button"
      tabIndex={0}
      onClick={(e) => {
        e.stopPropagation();
        setEditing(true);
      }}
      onKeyDown={(e) => {
        if (e.key === "Enter" || e.key === " ") {
          e.preventDefault();
          e.stopPropagation();
          setEditing(true);
        }
      }}
      title={title ?? "Click to rename"}
      className={`${textClassName} truncate text-left rounded px-1 -mx-1 hover:bg-gray-800 cursor-pointer ${
        value.trim() === "" ? "text-gray-500 italic" : "text-gray-300"
      }`}
    >
      {value.trim() === "" ? placeholder : value}
    </span>
  );
}
