// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { Trans, useTranslation } from "react-i18next";
import type { WorkflowSummary, WorkflowDefinition } from "../api/types";
import type { TerminalState, DynamicForwards } from "../hooks/useWorkflows";
import { IssueCard } from "./IssueCard";

interface Props {
  workflows: Record<string, WorkflowSummary>;
  orderKeys: string[];
  terminalStates: Record<string, TerminalState>;
  dynamicForwards: DynamicForwards;
  workflowDefs: WorkflowDefinition[];
  onRefresh: () => void;
  onShowDescription: (ticketKey: string, summary: string, description?: string) => void;
  onReport: (ticketKey: string) => void;
  onAddWorkflow: () => void;
  canAddWorkflow: boolean;
  repoExists: boolean;
  onSetupProject?: () => void;
  /**
   * When set, only workflows whose `workspace_name` matches this value are
   * shown. `null` (or omitted) shows all of the caller's items.
   */
  activeRepoName?: string | null;
}

export function WorkflowGrid({
  workflows,
  orderKeys,
  terminalStates,
  dynamicForwards,
  workflowDefs,
  onRefresh,
  onShowDescription,
  onReport,
  onAddWorkflow,
  canAddWorkflow,
  repoExists,
  onSetupProject,
  activeRepoName,
}: Props) {
  const { t } = useTranslation("dashboard");
  const fullList = orderKeys.map((k) => workflows[k]).filter(Boolean);
  const list = activeRepoName
    ? fullList.filter((w) => w.workspace_name === activeRepoName)
    : fullList;

  if (list.length === 0) {
    // No repo cloned yet — show project setup prompt
    if (!repoExists && onSetupProject) {
      return (
        <div className="text-center py-16">
          <p className="text-gray-500 text-sm mb-4">
            {t("emptyState.noProject")}
          </p>
          <button
            onClick={onSetupProject}
            className="text-sm px-4 py-2 rounded-lg bg-blue-600 text-white hover:bg-blue-500 transition-colors cursor-pointer"
          >
            {t("emptyState.setupProject")}
          </button>
        </div>
      );
    }

    // Active-repo filter is set but matches nothing → distinguish from
    // "you have no items at all".
    if (activeRepoName && fullList.length > 0) {
      return (
        <div className="text-center py-16">
          <p className="text-gray-500 text-sm mb-4">
            <Trans
              i18nKey="dashboard:emptyState.noItemsInRepo"
              values={{ repo: activeRepoName }}
              components={{ name: <span className="font-mono text-gray-400" /> }}
            />
          </p>
          {canAddWorkflow && (
            <button
              onClick={onAddWorkflow}
              className="text-sm px-4 py-2 rounded-lg bg-blue-600 text-white hover:bg-blue-500 transition-colors cursor-pointer"
            >
              {t("emptyState.newItem")}
            </button>
          )}
        </div>
      );
    }

    // Repo exists but no work items at all
    return (
      <div className="text-center py-16">
        <p className="text-gray-500 text-sm mb-4">
          {t("emptyState.noItems")}
        </p>
        {canAddWorkflow && (
          <button
            onClick={onAddWorkflow}
            className="text-sm px-4 py-2 rounded-lg bg-blue-600 text-white hover:bg-blue-500 transition-colors cursor-pointer"
          >
            {t("emptyState.newItem")}
          </button>
        )}
      </div>
    );
  }

  return (
    <div className="grid grid-cols-1 xl:grid-cols-2 gap-4">
      {list.map((w) => (
        <IssueCard
          key={w.ticket_key}
          workflow={w}
          terminalState={terminalStates[w.ticket_key]}
          dynamicForwards={dynamicForwards[w.ticket_key] || []}
          workflowDefs={workflowDefs}
          onRefresh={onRefresh}
          onShowDescription={onShowDescription}
          onReport={onReport}
        />
      ))}
      {canAddWorkflow && (
        <button
          onClick={onAddWorkflow}
          className="flex items-center justify-center h-16 border border-dashed border-gray-700 rounded-xl text-gray-500 hover:text-gray-300 hover:border-gray-600 transition-colors cursor-pointer text-lg"
        >
          +
        </button>
      )}
    </div>
  );
}
