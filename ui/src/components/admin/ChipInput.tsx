// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * A controlled tag/chip editor for string lists (keywords, labels, types).
 * Enter or comma commits the current entry; clicking a chip's × removes it;
 * Backspace on an empty field removes the last chip. Entries are trimmed and
 * de-duplicated; empty entries are ignored.
 */

import { useCallback, useState, type KeyboardEvent } from "react";
import { useTranslation } from "react-i18next";

interface ChipInputProps {
  id: string;
  label: string;
  values: string[];
  onChange: (next: string[]) => void;
  placeholder?: string;
  helpText?: string;
}

export function ChipInput({ id, label, values, onChange, placeholder, helpText }: ChipInputProps) {
  const { t } = useTranslation("config");
  const [entry, setEntry] = useState("");

  const commit = useCallback(() => {
    const trimmed = entry.trim();
    if (trimmed.length === 0) {
      setEntry("");
      return;
    }
    if (!values.includes(trimmed)) {
      onChange([...values, trimmed]);
    }
    setEntry("");
  }, [entry, values, onChange]);

  const remove = useCallback(
    (value: string) => onChange(values.filter((v) => v !== value)),
    [values, onChange],
  );

  const handleKeyDown = useCallback(
    (e: KeyboardEvent<HTMLInputElement>) => {
      if (e.key === "Enter" || e.key === ",") {
        e.preventDefault();
        commit();
      } else if (e.key === "Backspace" && entry === "" && values.length > 0) {
        e.preventDefault();
        onChange(values.slice(0, -1));
      }
    },
    [commit, entry, values, onChange],
  );

  return (
    <section className="flex flex-col gap-2">
      <label htmlFor={id} className="text-xs text-gray-400">
        {label}
      </label>
      <div className="flex flex-wrap items-center gap-2 bg-gray-950 border border-gray-700 rounded-lg px-3 py-2">
        {values.map((value) => (
          <span
            key={value}
            className="flex items-center gap-1 rounded-md bg-gray-800 px-2 py-0.5 text-xs text-gray-200"
          >
            {value}
            <button
              type="button"
              aria-label={t("chip.remove", { value })}
              onClick={() => remove(value)}
              className="text-gray-500 hover:text-gray-200 cursor-pointer"
            >
              ×
            </button>
          </span>
        ))}
        <input
          id={id}
          type="text"
          value={entry}
          onChange={(e) => setEntry(e.target.value)}
          onKeyDown={handleKeyDown}
          onBlur={commit}
          placeholder={values.length === 0 ? placeholder : ""}
          className="flex-1 min-w-[8rem] bg-transparent text-sm text-gray-200 outline-none"
        />
      </div>
      {helpText && <p className="text-xs text-gray-500">{helpText}</p>}
    </section>
  );
}
