// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Multiselect for a flow's `depends_on`. Options are the sibling flow names in
 * the current draft (never the flow being edited). Selected deps render as
 * removable chips; a dropdown adds the remaining ones.
 */

import { useState } from "react";

interface DependsOnSelectProps {
  options: string[];
  selected: string[];
  onChange: (next: string[]) => void;
}

export function DependsOnSelect({ options, selected, onChange }: DependsOnSelectProps) {
  const [open, setOpen] = useState(false);

  const available = options.filter((o) => !selected.includes(o));

  const add = (name: string) => {
    onChange([...selected, name]);
    setOpen(false);
  };
  const remove = (name: string) => {
    onChange(selected.filter((n) => n !== name));
  };

  return (
    <div className="relative">
      <div className="flex items-center flex-wrap gap-1.5 bg-gray-950 border border-gray-700 rounded px-2 py-1.5 min-h-[2.25rem]">
        {selected.map((name) => (
          <span
            key={name}
            className="inline-flex items-center gap-1 bg-gray-800 text-gray-300 px-1.5 py-0.5 rounded text-sm"
          >
            <span className="truncate max-w-[10rem]" title={name}>
              {name}
            </span>
            <button
              type="button"
              onClick={() => remove(name)}
              className="text-gray-500 hover:text-gray-200 cursor-pointer"
              aria-label={`Remove dependency ${name}`}
            >
              &times;
            </button>
          </span>
        ))}
        {selected.length === 0 && (
          <span className="text-sm text-gray-600">No dependencies</span>
        )}
        <button
          type="button"
          onClick={() => setOpen((v) => !v)}
          disabled={available.length === 0}
          className="ml-auto text-gray-500 hover:text-gray-300 disabled:text-gray-700 disabled:cursor-not-allowed cursor-pointer"
          aria-label="Add dependency"
          aria-expanded={open}
        >
          <svg className="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
            <path strokeLinecap="round" strokeLinejoin="round" d="M19 9l-7 7-7-7" />
          </svg>
        </button>
      </div>

      {open && available.length > 0 && (
        <>
          <div className="fixed inset-0 z-10" onClick={() => setOpen(false)} />
          <div className="absolute left-0 right-0 mt-1 z-20 bg-gray-900 border border-gray-700 rounded-lg shadow-lg max-h-48 overflow-y-auto py-1">
            {available.map((name) => (
              <button
                key={name}
                type="button"
                onClick={() => add(name)}
                className="block w-full text-left px-3 py-1.5 text-sm text-gray-300 hover:bg-gray-800 cursor-pointer truncate"
                title={name}
              >
                {name}
              </button>
            ))}
          </div>
        </>
      )}
    </div>
  );
}
