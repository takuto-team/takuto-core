// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { useEffect, useState } from "react";
import type { UserFlow } from "../api/flows";
import { FlowEditor } from "./FlowEditor";
import { EditableName } from "./EditableName";

interface FlowCardProps {
  flow: UserFlow;
  flows: UserFlow[];
  index: number;
  expanded: boolean;
  draggable: boolean;
  isDragging: boolean;
  onToggleExpand: () => void;
  onDelete: () => void;
  onSubmit: (next: UserFlow[]) => Promise<void>;
  onCancelEdit: () => void;
  onDragStart: () => void;
  onDrop: () => void;
  onDragEnd: () => void;
}

function stepCountLabel(n: number): string {
  return n === 1 ? "1 step" : `${n} steps`;
}

export function FlowCard({
  flow,
  flows,
  index,
  expanded,
  draggable,
  isDragging,
  onToggleExpand,
  onDelete,
  onSubmit,
  onCancelEdit,
  onDragStart,
  onDrop,
  onDragEnd,
}: FlowCardProps) {
  // Draft name is only meaningful while expanded; reset to the saved value on
  // every (re-)expansion or when the underlying flow's name changes. Cancel
  // therefore discards rename edits automatically.
  const [nameDraft, setNameDraft] = useState(flow.name);
  const [nameError, setNameError] = useState<string | null>(null);

  useEffect(() => {
    setNameDraft(flow.name);
    setNameError(null);
  }, [flow.name, expanded]);

  return (
    <div
      draggable={draggable && !expanded}
      onDragStart={(e) => {
        e.dataTransfer.effectAllowed = "move";
        onDragStart();
      }}
      onDragOver={(e) => e.preventDefault()}
      onDrop={(e) => {
        e.preventDefault();
        onDrop();
      }}
      onDragEnd={onDragEnd}
      className={`border border-gray-800 rounded-lg bg-gray-950 ${isDragging ? "opacity-40" : ""}`}
    >
      <button
        type="button"
        onClick={onToggleExpand}
        aria-expanded={expanded}
        aria-controls={`flow-${index}-editor`}
        className="flex items-center gap-3 px-3 py-2.5 w-full text-left cursor-pointer hover:bg-gray-900/50 rounded-lg"
      >
        <span
          className={`text-gray-600 select-none ${draggable && !expanded ? "cursor-grab" : "cursor-default"}`}
          title="Drag to reorder"
          aria-hidden="true"
        >
          ⠿
        </span>

        {expanded ? (
          <EditableName
            value={nameDraft}
            onChange={setNameDraft}
            placeholder="Untitled flow"
            textClassName="text-sm font-medium"
          />
        ) : (
          <span className="text-sm font-medium text-gray-200 truncate">{flow.name}</span>
        )}

        <span className="text-xs text-gray-500 whitespace-nowrap">
          {stepCountLabel(flow.steps.length)}
        </span>

        {flow.depends_on.length > 0 && (
          <span className="flex items-center gap-1 text-xs text-gray-500 min-w-0">
            <span className="whitespace-nowrap">depends on:</span>
            {flow.depends_on.map((dep) => (
              <span
                key={dep}
                title={dep}
                className="bg-gray-800 text-gray-400 px-1.5 py-0.5 rounded truncate max-w-[8rem]"
              >
                {dep}
              </span>
            ))}
          </span>
        )}

        <span className="ml-auto flex items-center gap-3">
          <span
            role="button"
            tabIndex={0}
            onClick={(e) => {
              e.stopPropagation();
              onDelete();
            }}
            onKeyDown={(e) => {
              if (e.key === "Enter" || e.key === " ") {
                e.preventDefault();
                e.stopPropagation();
                onDelete();
              }
            }}
            className="text-sm text-red-500/70 hover:text-red-400 cursor-pointer"
          >
            Delete
          </span>
          <span
            className="text-gray-500"
            title={expanded ? "Collapse" : "Expand"}
            aria-hidden="true"
          >
            <svg
              className={`w-4 h-4 transition-transform ${expanded ? "rotate-180" : ""}`}
              fill="none"
              viewBox="0 0 24 24"
              stroke="currentColor"
              strokeWidth={2}
            >
              <path strokeLinecap="round" strokeLinejoin="round" d="M19 9l-7 7-7-7" />
            </svg>
          </span>
        </span>
      </button>

      {expanded && nameError && (
        <p className="px-4 pt-2 text-sm text-red-400">{nameError}</p>
      )}

      {expanded && (
        <div id={`flow-${index}-editor`}>
          <FlowEditor
            flows={flows}
            editIndex={index}
            name={nameDraft}
            onNameError={setNameError}
            onSubmit={onSubmit}
            onCancel={onCancelEdit}
          />
        </div>
      )}
    </div>
  );
}
