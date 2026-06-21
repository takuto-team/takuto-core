// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/** Presentational header row of an IssueCard: ticket key, workspace badge, status, PR link. */

import { useTranslation } from "react-i18next";
import type { WorkflowSummary } from "../../api/types";
import { Label } from "../Label";
import { StatusBadge } from "../StatusBadge";
import type { StatusInfo } from "../StatusBadge";

interface Props {
  workflow: WorkflowSummary;
  status: StatusInfo;
  prUrl: string;
}

export function IssueCardHeader({ workflow: w, status, prUrl }: Props) {
  const { t } = useTranslation("dashboard");
  const href = w.issue_url || w.jira_browse_url;
  const prNumber = prUrl.match(/\/(\d+)\/?$/)?.[1] ?? "";
  return (
    <div className="flex items-center justify-between gap-3 min-w-0">
      <div className="flex items-center gap-2 min-w-0 flex-1">
        {href ? (
          <a
            href={href}
            target="_blank"
            rel="noopener noreferrer"
            className="font-mono text-base font-medium text-blue-400 hover:text-blue-300 transition-colors cursor-pointer"
          >
            {w.ticket_key}
          </a>
        ) : (
          <span className="font-mono text-base font-medium text-blue-400">{w.ticket_key}</span>
        )}
        {w.workspace_name && (
          <span
            className="text-[11px] px-1.5 py-0.5 rounded bg-gray-800 text-gray-400 border border-gray-700 shrink-0 truncate max-w-32"
            title={t("card.repositoryTitle", { name: w.workspace_name })}
          >
            {w.workspace_name}
          </span>
        )}
        <StatusBadge status={status} />
      </div>
      {prUrl && (
        <div className="flex-shrink-0">
          <Label variant={w.pr_merged ? "purple" : "info"} href={prUrl}>
            PR #{prNumber}
          </Label>
        </div>
      )}
    </div>
  );
}
