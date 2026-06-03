// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { useState } from "react";
import type { UserFlow } from "../api/flows";

interface FlowCardProps {
  flow: UserFlow;
  draggable: boolean;
  isDragging: boolean;
  onEdit: () => void;
  onDelete: () => void;
  onDragStart: () => void;
  onDrop: () => void;
  onDragEnd: () => void;
}

function stepCountLabel(n: number): string {
  return n === 1 ? "1 step" : `${n} steps`;
}

function firstLine(prompt: string): string {
  const line = prompt.split("\n", 1)[0] ?? "";
  return line.trim();
}

export function FlowCard({
  flow,
  draggable,
  isDragging,
  onEdit,
  onDelete,
  onDragStart,
  onDrop,
  onDragEnd,
}: FlowCardProps) {
  const [expanded, setExpanded] = useState(false);

  return (
    <div
      draggable={draggable}
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
      <div className="flex items-center gap-3 px-3 py-2.5">
        <span
          className={`text-gray-600 select-none ${draggable ? "cursor-grab" : "cursor-default"}`}
          title="Drag to reorder"
          aria-hidden="true"
        >
          ⠿
        </span>

        <span className="text-sm font-medium text-gray-200 truncate">{flow.name}</span>

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

        <div className="ml-auto flex items-center gap-3">
          <button
            type="button"
            onClick={onEdit}
            className="text-sm text-gray-400 hover:text-gray-200 cursor-pointer"
          >
            Edit
          </button>
          <button
            type="button"
            onClick={onDelete}
            className="text-sm text-red-500/70 hover:text-red-400 cursor-pointer"
          >
            Delete
          </button>
          <button
            type="button"
            onClick={() => setExpanded((v) => !v)}
            className="text-gray-500 hover:text-gray-300 cursor-pointer"
            title={expanded ? "Collapse" : "Expand"}
            aria-label={expanded ? "Collapse steps" : "Expand steps"}
            aria-expanded={expanded}
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
          </button>
        </div>
      </div>

      {expanded && (
        <div className="border-t border-gray-800 px-3 py-2.5 space-y-2">
          {flow.steps.map((step, i) => (
            <div key={`${step.name}-${i}`} className="text-sm">
              <div className="flex items-baseline gap-2 min-w-0">
                <span className="text-gray-500 whitespace-nowrap">{i + 1}.</span>
                <span className="text-gray-300 font-medium whitespace-nowrap">{step.name}</span>
                <span className="text-gray-500 font-mono text-xs truncate">{firstLine(step.prompt)}</span>
              </div>
              {step.skills.length > 0 && (
                <div className="ml-5 mt-1 flex items-center gap-1 flex-wrap">
                  <span className="text-xs text-gray-600">skills:</span>
                  {step.skills.map((skill, si) => (
                    <span
                      key={`${skill.name}-${si}`}
                      className="bg-gray-800 text-gray-400 px-1.5 py-0.5 rounded text-xs"
                    >
                      {skill.name}
                    </span>
                  ))}
                </div>
              )}
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
