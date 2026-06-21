// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
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
  /** Inline rename from the collapsed row (commits via blur or Enter). */
  onInlineRename: (newName: string) => Promise<void>;
  onCancelEdit: () => void;
  onDragStart: () => void;
  /**
   * Fired continuously while another card is dragged over this one.
   * `before === true` when the cursor is in the top half of the card
   * (insert above), `false` for the bottom half (insert below).
   */
  onDragOverCard: (before: boolean) => void;
  onDrop: () => void;
  onDragEnd: () => void;
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
  onInlineRename,
  onCancelEdit,
  onDragStart,
  onDragOverCard,
  onDrop,
  onDragEnd,
}: FlowCardProps) {
  const { t } = useTranslation("config");
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
      onDragOver={(e) => {
        e.preventDefault();
        const rect = e.currentTarget.getBoundingClientRect();
        onDragOverCard(e.clientY < rect.top + rect.height / 2);
      }}
      onDrop={(e) => {
        e.preventDefault();
        e.stopPropagation();
        onDrop();
      }}
      onDragEnd={onDragEnd}
      className={`border border-gray-800 rounded-lg bg-gray-950 ${isDragging ? "opacity-40" : ""}`}
    >
      <div
        role="button"
        tabIndex={0}
        onClick={onToggleExpand}
        onKeyDown={(e) => {
          // Activate only when focus is on the row itself; if a child (the
          // inline name input) is focused, let it consume the key. This is
          // why the row is a `<div role="button">` rather than a real
          // `<button>` — a native button activates on Space regardless of
          // which descendant holds focus, breaking inline name editing.
          if (e.target !== e.currentTarget) return;
          if (e.key === "Enter" || e.key === " ") {
            e.preventDefault();
            onToggleExpand();
          }
        }}
        aria-expanded={expanded}
        aria-controls={`flow-${index}-editor`}
        className="flex items-center gap-3 px-3 py-2.5 w-full text-left cursor-pointer hover:bg-gray-900/50 rounded-lg"
      >
        <span
          className={`text-gray-600 select-none ${draggable && !expanded ? "cursor-grab" : "cursor-default"}`}
          title={t("flows.card.dragToReorder")}
          aria-hidden="true"
        >
          ⠿
        </span>

        <EditableName
          value={nameDraft}
          onChange={setNameDraft}
          onCommit={async (next) => {
            const trimmed = next.trim();
            if (trimmed === "" || trimmed === flow.name) {
              setNameDraft(flow.name);
              return;
            }
            try {
              await onInlineRename(trimmed);
            } catch {
              setNameDraft(flow.name);
            }
          }}
          placeholder={t("flows.untitled")}
          textClassName="text-sm font-medium"
        />

        <span className="text-xs text-gray-500 whitespace-nowrap">
          {t("flows.card.stepCount", { count: flow.steps.length })}
        </span>

        {flow.depends_on.length > 0 && (
          <span className="flex items-center gap-1 text-xs text-gray-500 min-w-0">
            <span className="whitespace-nowrap">{t("flows.card.dependsOn")}</span>
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
            {t("actions.delete")}
          </span>
          <span
            className="text-gray-500"
            title={expanded ? t("flows.card.collapse") : t("flows.card.expand")}
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
      </div>

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
